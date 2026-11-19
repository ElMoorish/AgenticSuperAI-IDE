import { describe, expect, it } from "vitest";
import {
  contextBreakdown,
  initialAgentState,
  MAX_DETAIL_CHARS,
  MAX_MESSAGES,
  MAX_MESSAGE_CHARS,
  reduceAgent,
  type AgentAction,
  type AgentState,
} from "../agent/agentStore";

const run = (events: AgentAction[], s: AgentState = initialAgentState()): AgentState => events.reduce(reduceAgent, s);

describe("reduceAgent", () => {
  it("reducer_adds_step_on_tool_call_and_updates_status", () => {
    let s = run([
      { type: "promptSent", text: "fix it" },
      { type: "toolCall", id: "t1", title: "read_file main.cpp", kind: "read", status: "pending" },
    ]);
    expect(s.steps).toEqual([{ id: "t1", title: "read_file main.cpp", kind: "read", status: "todo", seq: 2 }]);
    s = reduceAgent(s, { type: "toolUpdate", id: "t1", status: "in_progress" });
    expect(s.steps[0].status).toBe("run");
    s = reduceAgent(s, { type: "toolUpdate", id: "t1", status: "completed", content: "ok" });
    expect(s.steps[0]).toMatchObject({ status: "done", detail: "ok" });
    s = reduceAgent(s, { type: "toolCall", id: "t2", title: "run build", kind: "execute", status: "pending" });
    s = reduceAgent(s, { type: "toolUpdate", id: "t2", status: "failed", content: "exit 1" });
    expect(s.steps.map((x) => x.status)).toEqual(["done", "failed"]);
    // an update for an unknown tool call is not invented into a step
    expect(reduceAgent(s, { type: "toolUpdate", id: "zzz", status: "completed" }).steps).toHaveLength(2);
    // a repeated toolCall for the same id updates it instead of duplicating
    expect(reduceAgent(s, { type: "toolCall", id: "t1", title: "read_file main.cpp", kind: "read", status: "completed" }).steps).toHaveLength(2);
  });

  it("reducer_truncates_long_tool_content_with_notice", () => {
    const big = "x".repeat(MAX_DETAIL_CHARS * 50);
    const s = run([
      { type: "toolCall", id: "t1", title: "cat huge.log", kind: "read", status: "completed", content: big },
    ]);
    const d = s.steps[0].detail ?? "";
    expect(d.length).toBeLessThan(MAX_DETAIL_CHARS + 200);
    expect(d).toMatch(/more characters not shown/);
    expect(s.steps[0].truncated).toBe(true);
    const u = reduceAgent(s, { type: "toolUpdate", id: "t1", status: "completed", content: big });
    expect((u.steps[0].detail ?? "").length).toBeLessThan(MAX_DETAIL_CHARS + 200);
  });

  it("reducer_sets_waiting_on_approval_and_clears_on_turn_end", () => {
    let s = run([
      { type: "promptSent", text: "go" },
      { type: "approval", id: 7, kind: "edit", title: "Edit main.cpp", old: "a", new: "b" },
    ]);
    expect(s.turn).toBe("waiting");
    expect(s.approvals.map((a) => a.id)).toEqual([7]);
    // the host replays pending approvals on (re)subscribe: no duplicates
    s = reduceAgent(s, { type: "approval", id: 7, kind: "edit", title: "Edit main.cpp", old: "a", new: "b" });
    expect(s.approvals).toHaveLength(1);
    s = reduceAgent(s, { type: "approvalResolved", id: 7 });
    expect(s.approvals).toHaveLength(0);
    expect(s.turn).toBe("running");
    s = reduceAgent(s, { type: "approval", id: 8, kind: "command", title: "Run", command: "cl /EHsc main.cpp" });
    s = reduceAgent(s, { type: "turnEnd", stopReason: "end_turn" });
    expect(s.turn).toBe("idle");
    expect(s.approvals).toEqual([]);
  });

  it("reducer_banner_on_crash_and_clears_pending_approvals", () => {
    let s = run([
      { type: "promptSent", text: "go" },
      { type: "toolCall", id: "t1", title: "build", kind: "execute", status: "in_progress" },
      { type: "approval", id: 3, kind: "command", title: "Run", command: "x" },
      { type: "agentState", state: "crashed" },
    ]);
    expect(s.banner).toMatch(/agent stopped|crashed/i);
    expect(s.approvals).toEqual([]);
    expect(s.turn).toBe("idle");
    expect(s.steps[0].status).toBe("failed"); // a running step cannot stay "running" forever
    s = reduceAgent(s, { type: "agentState", state: "restarted" });
    expect(s.banner).toMatch(/restarted/i);
    s = reduceAgent(s, { type: "agentState", state: "gave_up" });
    expect(s.banner).toMatch(/gave up|could not be restarted/i);
    s = reduceAgent(s, { type: "dismissBanner" });
    expect(s.banner).toBeUndefined();
    // a running agent does not by itself raise a banner
    expect(reduceAgent(initialAgentState(), { type: "agentState", state: "running" }).banner).toBeUndefined();
  });

  it("reducer_caps_message_history_for_render", () => {
    let s = run([{ type: "promptSent", text: "go" }]);
    for (let i = 0; i < 100_000; i++) s = reduceAgent(s, { type: "chunk", text: "word " });
    const total = s.messages.reduce((n, m) => n + m.text.length, 0);
    expect(total).toBeLessThanOrEqual(MAX_MESSAGE_CHARS + 200);
    expect(s.messages.at(-1)?.trimmed).toBe(true);
    // many turns: the message list itself is capped
    for (let i = 0; i < MAX_MESSAGES * 3; i++) s = run([{ type: "promptSent", text: `q${i}` }, { type: "chunk", text: "a" }, { type: "turnEnd", stopReason: "end_turn" }], s);
    expect(s.messages.length).toBeLessThanOrEqual(MAX_MESSAGES);
    expect(s.messages.at(-1)?.text).toBe("a");
  });

  it("chunks stream into one assistant message per turn; a prompt adds the user message and keeps the earlier steps", () => {
    let s = run([
      { type: "promptSent", text: "hello" },
      { type: "chunk", text: "Hel" },
      { type: "chunk", text: "lo!" },
    ]);
    expect(s.messages.map((m) => [m.role, m.text])).toEqual([["user", "hello"], ["assistant", "Hello!"]]);
    expect(s.turn).toBe("running");
    s = run([
      { type: "toolCall", id: "t1", title: "x", kind: "read", status: "completed" },
      { type: "turnEnd", stopReason: "end_turn" },
      { type: "promptSent", text: "again" },
    ], s);
    expect(s.steps).toHaveLength(1); // earlier steps stay in the conversation
    expect(s.messages.at(-1)).toMatchObject({ role: "user", text: "again" });
    // a chunk after a new prompt starts a NEW assistant message
    s = reduceAgent(s, { type: "chunk", text: "ok" });
    expect(s.messages.at(-1)).toMatchObject({ role: "assistant", text: "ok" });
  });

  it("steps and messages share one arrival order, and a restarted agent reusing tool ids does not overwrite old steps", () => {
    let s = run([
      { type: "promptSent", text: "q1" },
      { type: "chunk", text: "looking" },
      { type: "toolCall", id: "tc1", title: "list_dir .", kind: "read", status: "completed" },
      { type: "chunk", text: "done" },
      { type: "turnEnd", stopReason: "end_turn" },
    ]);
    const order = [...s.messages.map((m) => m.id), ...s.steps.map((x) => x.seq)].sort((a, b) => a - b);
    expect(new Set(order).size).toBe(order.length); // no two items share a sequence number
    expect(s.steps[0].seq).toBeGreaterThan(s.messages[1].id); // the step came after "looking"
    expect(s.steps[0].seq).toBeLessThan(s.messages[2].id); // and before the text that followed it
    // new turn, the agent restarted so it uses "tc1" again: this is a new step
    s = run([
      { type: "promptSent", text: "q2" },
      { type: "toolCall", id: "tc1", title: "read_file a.py", kind: "read", status: "pending" },
    ], s);
    expect(s.steps.map((x) => x.title)).toEqual(["list_dir .", "read_file a.py"]);
  });

  it("turnEnd finishes running steps and explains cancelled or failed turns", () => {
    let s = run([
      { type: "promptSent", text: "go" },
      { type: "toolCall", id: "t1", title: "build", kind: "execute", status: "in_progress" },
      { type: "turnEnd", stopReason: "cancelled" },
    ]);
    expect(s.turn).toBe("idle");
    expect(s.steps[0].status).toBe("failed");
    expect(s.messages.at(-1)).toMatchObject({ role: "notice" });
    s = run([{ type: "promptSent", text: "go" }, { type: "turnEnd", stopReason: "error" }]);
    expect(s.messages.at(-1)?.role).toBe("notice");
    // a normal end adds no notice
    s = run([{ type: "promptSent", text: "go" }, { type: "chunk", text: "done" }, { type: "turnEnd", stopReason: "end_turn" }]);
    expect(s.messages.at(-1)?.role).toBe("assistant");
  });

  it("usage and gpu are stored; unrelated events return the same state object", () => {
    let s = run([
      { type: "usage", promptTokens: 6200, completionTokens: 300, tokensPerSecond: 38.5 },
      { type: "gpu", usedMiB: 5800, totalMiB: 8188 },
    ]);
    expect(s.usage).toEqual({ prompt: 6200, completion: 300, tps: 38.5 });
    expect(s.gpu).toEqual({ usedMiB: 5800, totalMiB: 8188 });
    for (const ev of [
      { type: "fsChanged", paths: ["a"], overflow: false },
      { type: "applyEdit", id: 1, rel: "a", newText: "b" },
      { type: "agentTerminalOutput", id: 1, chunk: "x" },
      { type: "agentTerminalTruncated", id: 1 },
      { type: "agentTerminalExit", id: 1, code: 0 },
      { type: "userTerminalExit", id: 1, code: 0 },
    ] as AgentAction[]) {
      expect(reduceAgent(s, ev)).toBe(s);
    }
    s = reduceAgent(s, { type: "usage", promptTokens: 1, completionTokens: 1, tokensPerSecond: Number.NaN });
    expect(s.usage.tps).toBe(0); // never show NaN
  });

  it("promptFailed shows a readable banner and returns to idle; promptDone settles a turn the host never ended", () => {
    let s = run([{ type: "promptSent", text: "go" }, { type: "promptFailed", code: "agent_missing", message: "agent-core executable not found" }]);
    expect(s.turn).toBe("idle");
    expect(s.banner).toMatch(/agent-core executable not found/);
    s = run([{ type: "promptSent", text: "go" }, { type: "promptDone" }]);
    expect(s.turn).toBe("idle");
    // promptDone must not end a turn that is waiting on an approval the user still has to answer
    s = run([{ type: "promptSent", text: "go" }, { type: "approval", id: 1, kind: "edit", title: "t" }, { type: "promptDone" }]);
    expect(s.turn).toBe("waiting");
  });
});

describe("contextBreakdown", () => {
  it("splits usage into system, chat and tool segments against the configured context size", () => {
    const s = run([
      { type: "promptSent", text: "a".repeat(400) },
      { type: "toolCall", id: "t1", title: "read", kind: "read", status: "completed", content: "b".repeat(800) },
      { type: "chunk", text: "c".repeat(400) },
      { type: "usage", promptTokens: 1000, completionTokens: 100, tokensPerSecond: 10 },
    ]);
    const c = contextBreakdown(s, 16384);
    expect(c.total).toBe(16384);
    expect(c.used).toBe(1100);
    expect(c.chat).toBe(200); // ~4 chars per token
    expect(c.tool).toBe(200);
    expect(c.system).toBe(700); // what usage says is in the prompt beyond chat and tool estimates
    expect(c.system + c.chat + c.tool).toBe(c.used);
    expect(c.fraction).toBeCloseTo(1100 / 16384, 5);
  });

  it("falls back to estimates before any usage arrives and never divides by zero", () => {
    const s = run([{ type: "promptSent", text: "a".repeat(40) }]);
    const c = contextBreakdown(s, 8192);
    expect(c.used).toBe(10);
    expect(c.system).toBe(0);
    expect(contextBreakdown(s, 0).fraction).toBe(0);
    expect(contextBreakdown(s, 100).fraction).toBeLessThanOrEqual(1);
  });
});
