/**
 * Agent panel state. `reduceAgent` is a pure reducer over host UiEvents plus a few local actions;
 * `useAgent` wires it to zustand and the ipc commands.
 *
 * Bounded by construction (Review Focus 4): tool output is truncated with a notice, the streaming
 * assistant message is trimmed from the front once it passes MAX_MESSAGE_CHARS, and the message
 * list keeps only the newest MAX_MESSAGES entries. A 100k-line tool result or a runaway model
 * therefore cannot grow memory or make the panel slow.
 */
import { create } from "zustand";
import {
  agentCancel,
  agentPrompt,
  normalizeError,
  resolveApproval,
  type AgentMode,
  type UiEvent,
} from "../ipc";
import { dispatchUiEvent } from "../events";

export const MAX_DETAIL_CHARS = 2000;
export const MAX_MESSAGE_CHARS = 100_000;
export const MAX_MESSAGES = 200;
/** Tool steps kept for the whole conversation (they interleave with messages); the oldest go first. */
export const MAX_STEPS = 400;
/** Default model context window until the settings UI (Task 13) provides one. */
export const DEFAULT_CONTEXT_TOKENS = 16384;

export type StepStatus = "todo" | "run" | "done" | "failed";
export interface Step {
  id: string;
  title: string;
  kind: string;
  status: StepStatus;
  /** Arrival order, from the same counter as message ids, so the chat can interleave steps and messages. */
  seq: number;
  detail?: string;
  /** The detail was cut to MAX_DETAIL_CHARS. */
  truncated?: boolean;
}

export interface Message {
  id: number;
  role: "user" | "assistant" | "notice";
  text: string;
  /** The beginning of this message was dropped to stay within MAX_MESSAGE_CHARS. */
  trimmed?: boolean;
}

export type Approval = Omit<Extract<UiEvent, { type: "approval" }>, "type">;

export interface AgentState {
  turn: "idle" | "running" | "waiting";
  steps: Step[];
  messages: Message[];
  usage: { prompt: number; completion: number; tps: number };
  approvals: Approval[];
  gpu?: { usedMiB: number; totalMiB: number };
  banner?: string;
  /** The last message is the assistant message of the turn in progress (chunks append to it). */
  streaming: boolean;
  nextId: number;
  /** First sequence number of the turn in progress; tool ids are only matched within it. */
  turnStart: number;
}

/** UiEvents plus the actions that only exist inside the webview. */
export type AgentAction =
  | UiEvent
  | { type: "promptSent"; text: string }
  | { type: "promptFailed"; code: string; message: string }
  | { type: "promptDone" }
  | { type: "approvalResolved"; id: number }
  | { type: "notify"; message: string }
  | { type: "dismissBanner" };

export const initialAgentState = (): AgentState => ({
  turn: "idle",
  steps: [],
  messages: [],
  usage: { prompt: 0, completion: 0, tps: 0 },
  approvals: [],
  streaming: false,
  nextId: 1,
  turnStart: 1,
});

const STATUS: Record<string, StepStatus> = { pending: "todo", in_progress: "run", completed: "done", failed: "failed" };
const stepStatus = (s: string): StepStatus => STATUS[s] ?? "todo";

function cut(text: string | undefined): { detail?: string; truncated?: boolean } {
  if (text === undefined) return {};
  if (text.length <= MAX_DETAIL_CHARS) return { detail: text };
  const more = text.length - MAX_DETAIL_CHARS;
  return { detail: `${text.slice(0, MAX_DETAIL_CHARS)}\n… ${more.toLocaleString("en-US")} more characters not shown`, truncated: true };
}

function pushMessage(s: AgentState, role: Message["role"], text: string): AgentState {
  const t = text.length > MAX_MESSAGE_CHARS ? text.slice(text.length - MAX_MESSAGE_CHARS) : text;
  let messages = [...s.messages, { id: s.nextId, role, text: t, ...(t.length < text.length ? { trimmed: true } : {}) }];
  if (messages.length > MAX_MESSAGES) messages = messages.slice(messages.length - MAX_MESSAGES);
  return { ...s, messages, nextId: s.nextId + 1 };
}

/** Steps that were in flight when the turn or the agent ended cannot stay "running". */
const settle = (steps: Step[], running: StepStatus): Step[] =>
  steps.some((x) => x.status === "run") ? steps.map((x) => (x.status === "run" ? { ...x, status: running } : x)) : steps;

const END_NOTICE: Record<string, string> = {
  cancelled: "Stopped.",
  error: "The turn failed. Check that the model server is running.",
  max_tokens: "Stopped: the model reached its token limit.",
  max_turn_requests: "Stopped: the agent reached its step limit for one request.",
  refusal: "The model declined this request.",
};

export function reduceAgent(s: AgentState, ev: AgentAction): AgentState {
  switch (ev.type) {
    case "promptSent": {
      const next = pushMessage(s, "user", ev.text);
      return { ...next, turn: "running", approvals: [], banner: undefined, streaming: false, turnStart: next.nextId };
    }
    case "chunk": {
      const last = s.messages[s.messages.length - 1];
      if (s.streaming && last && last.role === "assistant") {
        let text = last.text + ev.text;
        let trimmed = last.trimmed;
        if (text.length > MAX_MESSAGE_CHARS) {
          // Trim well below the cap so the (flattening) slice is amortised over many chunks.
          text = text.slice(text.length - Math.floor(MAX_MESSAGE_CHARS * 0.9));
          trimmed = true;
        }
        const messages = s.messages.slice();
        messages[messages.length - 1] = { ...last, text, ...(trimmed ? { trimmed } : {}) };
        return { ...s, messages };
      }
      return { ...pushMessage(s, "assistant", ev.text), streaming: true };
    }
    case "toolCall": {
      const i = s.steps.findIndex((x) => x.id === ev.id && x.seq >= s.turnStart);
      if (i < 0) {
        // Tool ids restart every session ("tc1"...), so an id seen in an earlier turn is a new step now.
        const step: Step = { id: ev.id, title: ev.title, kind: ev.kind, status: stepStatus(ev.status), seq: s.nextId, ...cut(ev.content) };
        let steps = [...s.steps, step];
        if (steps.length > MAX_STEPS) steps = steps.slice(steps.length - MAX_STEPS);
        return { ...s, steps, streaming: false, nextId: s.nextId + 1 };
      }
      const steps = s.steps.slice();
      steps[i] = { ...steps[i], title: ev.title, kind: ev.kind, status: stepStatus(ev.status), ...cut(ev.content) };
      return { ...s, steps };
    }
    case "toolUpdate": {
      const i = s.steps.findIndex((x) => x.id === ev.id && x.seq >= s.turnStart);
      if (i < 0) return s;
      const steps = s.steps.slice();
      steps[i] = { ...steps[i], status: stepStatus(ev.status), ...(ev.content !== undefined ? cut(ev.content) : {}) };
      return { ...s, steps };
    }
    case "usage":
      return {
        ...s,
        usage: {
          prompt: ev.promptTokens,
          completion: ev.completionTokens,
          tps: Number.isFinite(ev.tokensPerSecond) ? ev.tokensPerSecond : 0,
        },
      };
    case "gpu":
      return { ...s, gpu: { usedMiB: ev.usedMiB, totalMiB: ev.totalMiB } };
    case "approval": {
      if (s.approvals.some((a) => a.id === ev.id)) return s; // the host replays pending ones on subscribe
      const { type: _t, ...a } = ev;
      void _t;
      return { ...s, approvals: [...s.approvals, a], turn: "waiting" };
    }
    case "approvalResolved": {
      const approvals = s.approvals.filter((a) => a.id !== ev.id);
      if (approvals.length === s.approvals.length) return s;
      return { ...s, approvals, turn: approvals.length === 0 && s.turn === "waiting" ? "running" : s.turn };
    }
    case "agentState": {
      if (ev.state === "running") return s;
      if (ev.state === "restarted") return { ...s, banner: "The agent restarted. You can send your request again." };
      const banner =
        ev.state === "crashed"
          ? "The agent stopped unexpectedly. It is being restarted."
          : "The agent could not be restarted and gave up after repeated crashes. Check the model server, then reopen the folder.";
      return { ...s, banner, approvals: [], turn: "idle", steps: settle(s.steps, "failed"), streaming: false };
    }
    case "turnEnd": {
      const failed = ev.stopReason !== "end_turn";
      let next: AgentState = { ...s, turn: "idle", approvals: [], streaming: false, steps: settle(s.steps, failed ? "failed" : "done") };
      const notice = END_NOTICE[ev.stopReason] ?? (failed ? `The turn ended (${ev.stopReason}).` : undefined);
      if (notice) next = pushMessage(next, "notice", notice);
      return next;
    }
    case "promptFailed":
      return { ...s, turn: "idle", approvals: [], streaming: false, banner: ev.message || `The request failed (${ev.code}).`, steps: settle(s.steps, "failed") };
    case "promptDone":
      return s.turn === "running" ? { ...s, turn: "idle", streaming: false } : s;
    case "notify":
      return { ...s, banner: ev.message };
    case "dismissBanner":
      return s.banner === undefined ? s : { ...s, banner: undefined };
    default:
      return s;
  }
}

// ---- context gauge ----
export interface ContextBreakdown {
  system: number;
  chat: number;
  tool: number;
  used: number;
  total: number;
  fraction: number;
}

const CHARS_PER_TOKEN = 4;

/** Segment the prompt into system / chat / tool tokens from usage plus character estimates. */
export function contextBreakdown(s: AgentState, total: number): ContextBreakdown {
  const chatChars = s.messages.reduce((n, m) => (m.role === "notice" ? n : n + m.text.length), 0);
  const toolChars = s.steps.reduce((n, x) => n + (x.detail?.length ?? 0), 0);
  const chat = Math.round(chatChars / CHARS_PER_TOKEN);
  const tool = Math.round(toolChars / CHARS_PER_TOKEN);
  const reported = s.usage.prompt + s.usage.completion;
  const used = reported > 0 ? reported : chat + tool;
  const system = reported > 0 ? Math.max(0, used - chat - tool) : 0;
  return { system, chat, tool, used, total, fraction: total > 0 ? Math.min(1, used / total) : 0 };
}

// ---- zustand store ----
export const CONTEXT_KEY = "asa.contextSize";
export const clampContext = (n: number): number =>
  Number.isFinite(n) ? Math.min(1_048_576, Math.max(2048, Math.round(n))) : DEFAULT_CONTEXT_TOKENS;
function loadContextSize(): number {
  try {
    const raw = localStorage.getItem(CONTEXT_KEY);
    if (raw !== null) return clampContext(Number(raw));
  } catch { /* ignore */ }
  return DEFAULT_CONTEXT_TOKENS;
}

interface AgentStore {
  state: AgentState;
  contextSize: number;
  mode: AgentMode;
  dispatch: (a: AgentAction) => void;
  setMode: (m: AgentMode) => void;
  setContextSize: (n: number) => void;
  send: (text: string) => Promise<void>;
  cancel: () => Promise<void>;
  resolve: (id: number, accept: boolean) => Promise<void>;
}

export const useAgent = create<AgentStore>((set, get) => ({
  state: initialAgentState(),
  contextSize: loadContextSize(),
  mode: "build",
  dispatch: (a) => set((st) => {
    const next = reduceAgent(st.state, a);
    return next === st.state ? st : { state: next };
  }),
  setMode: (mode) => set({ mode }),
  setContextSize: (n) => {
    const v = clampContext(n);
    set({ contextSize: v });
    try { localStorage.setItem(CONTEXT_KEY, String(v)); } catch { /* ignore */ }
  },
  send: async (text) => {
    const t = text.trim();
    const { state, dispatch, mode } = get();
    if (!t || state.turn !== "idle") return;
    dispatch({ type: "promptSent", text: t });
    try {
      // `onEvent` REPLACES the host listener, so it must be the shared hub fan-out.
      await agentPrompt(mode, t, dispatchUiEvent);
    } catch (e) {
      const err = normalizeError(e);
      dispatch({ type: "promptFailed", code: err.code, message: err.message });
    } finally {
      dispatch({ type: "promptDone" });
    }
  },
  cancel: async () => {
    try {
      await agentCancel(); // the turn ends through the host's TurnEnd{cancelled}
    } catch {
      /* nothing running */
    }
  },
  resolve: async (id, accept) => {
    const { dispatch } = get();
    try {
      await resolveApproval(id, accept);
    } catch (e) {
      const err = normalizeError(e);
      // A stale/replayed id is simply gone; any other failure (edit conflict, io) is shown.
      if (err.code !== "unknown_approval") dispatch({ type: "notify", message: `Could not apply your decision: ${err.message}` });
    }
    dispatch({ type: "approvalResolved", id });
  },
}));
