/**
 * Typed wrappers over the Tauri command surface (apps/shell/src-tauri/src/{commands,state}.rs).
 * Every function fails gracefully outside Tauri (plain browser, Vitest): it rejects with an
 * `IpcError` whose code is "no_tauri" instead of throwing a TypeError.
 * Errors from Rust arrive as `{code, message}`; see `normalizeError`.
 */
import { Channel, invoke } from "@tauri-apps/api/core";

/** Must match `tauri::generate_handler!` in main.rs (checked by a test). */
export const COMMANDS = [
  "pick_folder", "open_folder", "list_dir", "read_file", "save_file", "subscribe_events",
  "agent_prompt", "agent_cancel", "resolve_approval", "probe_endpoint", "detect_servers",
  "get_settings", "set_settings", "start_user_terminal", "terminal_write", "terminal_resize",
  "close_terminal",
] as const;
export type CommandName = (typeof COMMANDS)[number];

/** Known codes: no_workspace escapes not_found io conflict binary too_large bad_request busy
 *  agent_missing agent unknown_approval terminal; plus client-side "no_tauri" and "unknown". */
export class IpcError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "IpcError";
    this.code = code;
  }
}

export function normalizeError(e: unknown): IpcError {
  if (e instanceof IpcError) return e;
  if (typeof e === "string") return new IpcError("unknown", e);
  if (e instanceof Error) return new IpcError("unknown", e.message);
  if (e && typeof e === "object") {
    const o = e as { code?: unknown; message?: unknown };
    if (typeof o.code === "string") return new IpcError(o.code, typeof o.message === "string" ? o.message : o.code);
  }
  return new IpcError("unknown", "unknown error");
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function call<T>(cmd: CommandName, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new IpcError("no_tauri", `${cmd}: not running inside the desktop app`);
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw normalizeError(e);
  }
}

// ---- DTOs (mirror state.rs; JSON field names as serialized) ----
export interface WorkspaceInfo { root: string; name: string }
export interface DirEntry { name: string; rel: string; isDir: boolean; size?: number | null }
export interface Stamp { mtimeMs: number; hash: string }
export interface ReadFileResult { text: string; stamp: Stamp }
export interface ProbeResult { ok: boolean; models: string[]; error: string | null }
export interface DetectedServer { name: string; url: string }
/** snake_case `font_size` exactly as ide-host serializes; Rust rejects unknown keys on set. */
export interface Settings {
  endpoint: string;
  model: string;
  ignore: string[];
  theme: string;
  font_size: number;
  recent: string[];
}
export type AgentMode = "plan" | "build";

export type UiEvent =
  | { type: "chunk"; text: string }
  | { type: "toolCall"; id: string; title: string; kind: string; status: string; content?: string }
  | { type: "toolUpdate"; id: string; status: string; content?: string }
  | { type: "usage"; promptTokens: number; completionTokens: number; tokensPerSecond: number }
  | { type: "approval"; id: number; kind: "edit" | "command"; title: string; old?: string; new?: string; command?: string }
  | { type: "applyEdit"; id: number; rel: string; newText: string }
  | { type: "agentState"; state: "running" | "crashed" | "restarted" | "gave_up" }
  | { type: "turnEnd"; stopReason: string }
  | { type: "fsChanged"; paths: string[]; overflow: boolean }
  | { type: "gpu"; usedMiB: number; totalMiB: number }
  | { type: "agentTerminalOutput"; id: number; chunk: string }
  | { type: "agentTerminalTruncated"; id: number }
  | { type: "agentTerminalExit"; id: number; code: number | null }
  | { type: "userTerminalExit"; id: number; code: number | null };

export function assertNever(x: never): never {
  throw new Error(`unhandled variant: ${JSON.stringify(x)}`);
}

/** Exhaustive over UiEvent: adding a Rust variant here without handling it fails to compile. */
export function describeEvent(ev: UiEvent): string {
  switch (ev.type) {
    case "chunk": return "chunk";
    case "toolCall": return "toolCall";
    case "toolUpdate": return "toolUpdate";
    case "usage": return "usage";
    case "approval": return "approval";
    case "applyEdit": return "applyEdit";
    case "agentState": return "agentState";
    case "turnEnd": return "turnEnd";
    case "fsChanged": return "fsChanged";
    case "gpu": return "gpu";
    case "agentTerminalOutput": return "agentTerminalOutput";
    case "agentTerminalTruncated": return "agentTerminalTruncated";
    case "agentTerminalExit": return "agentTerminalExit";
    case "userTerminalExit": return "userTerminalExit";
    default: return assertNever(ev);
  }
}

// ---- commands ----
export const pickFolder = () => call<string | null>("pick_folder");
export const openFolder = (path: string) => call<WorkspaceInfo>("open_folder", { path });
export const listDir = (rel: string) => call<DirEntry[]>("list_dir", { rel });
export const readFile = (rel: string) => call<ReadFileResult>("read_file", { rel });
/** Rejects with code "conflict" when the stamp is stale. */
export const saveFile = (rel: string, text: string, stamp: Stamp) => call<Stamp>("save_file", { rel, text, stamp });

/**
 * Register the single listener for ALL UiEvents (call once at startup). The host replays the
 * latest agentState, an undelivered turnEnd and every pending approval on (re)subscribe, so
 * consumers MUST dedupe approvals by id. Returns the channel (kept alive by the caller).
 */
export async function subscribeEvents(onEvent: (ev: UiEvent) => void): Promise<Channel<UiEvent>> {
  const ch = new Channel<UiEvent>();
  ch.onmessage = onEvent;
  await call<void>("subscribe_events", { onEvent: ch });
  return ch;
}
/** `onEvent` REPLACES the current listener; pass the same handler used for subscribeEvents. */
export async function agentPrompt(mode: AgentMode, text: string, onEvent: (ev: UiEvent) => void): Promise<void> {
  const ch = new Channel<UiEvent>();
  ch.onmessage = onEvent;
  await call<void>("agent_prompt", { mode, text, onEvent: ch });
}
export const agentCancel = () => call<void>("agent_cancel");
/** May reject with "unknown_approval" (stale/replayed id: ignore), "conflict" or "io" (edit write failed). */
export const resolveApproval = (id: number, accept: boolean) => call<void>("resolve_approval", { id, accept });
export const probeEndpoint = (url: string) => call<ProbeResult>("probe_endpoint", { url });
export const detectServers = () => call<DetectedServer[]>("detect_servers");
export const getSettings = () => call<Settings>("get_settings");
export const SETTINGS_KEYS = ["endpoint", "model", "ignore", "theme", "font_size", "recent"] as const;
/** Rust rejects unknown keys, so only the six known keys are ever sent. */
export function pickSettings(s: Settings): Settings {
  const o = s as unknown as Record<string, unknown>;
  const out: Record<string, unknown> = {};
  for (const k of SETTINGS_KEYS) out[k] = o[k];
  return out as unknown as Settings;
}
export const setSettings = (settings: Settings) => call<Settings>("set_settings", { settings: pickSettings(settings) });

/** Output arrives as byte arrays on `onOutput`. Returns the terminal id (max 8 live). */
export async function startUserTerminal(rows: number, cols: number, onOutput: (bytes: number[]) => void): Promise<number> {
  const ch = new Channel<number[]>();
  ch.onmessage = onOutput;
  return call<number>("start_user_terminal", { rows, cols, onOutput: ch });
}
export const terminalWrite = (id: number, data: number[]) => call<void>("terminal_write", { id, data });
export const terminalResize = (id: number, rows: number, cols: number) => call<void>("terminal_resize", { id, rows, cols });
export const closeTerminal = (id: number) => call<void>("close_terminal", { id });

// ---- window controls (custom title bar); no-ops outside Tauri ----
export async function windowControl(action: "minimize" | "toggleMaximize" | "close"): Promise<void> {
  if (!isTauri()) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const w = getCurrentWindow();
    if (action === "minimize") await w.minimize();
    else if (action === "toggleMaximize") await w.toggleMaximize();
    else await w.close();
  } catch (e) {
    throw normalizeError(e);
  }
}
