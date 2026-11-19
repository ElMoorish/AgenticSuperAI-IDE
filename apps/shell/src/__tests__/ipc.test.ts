import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { COMMANDS, IpcError, describeEvent, isTauri, normalizeError, getSettings, type UiEvent } from "../ipc";

describe("ipc", () => {
  it("normalizes {code,message} errors", () => {
    const e = normalizeError({ code: "conflict", message: "disk changed" });
    expect(e).toBeInstanceOf(IpcError);
    expect(e.code).toBe("conflict");
    expect(e.message).toBe("disk changed");
  });

  it("normalizes string, Error and unknown errors", () => {
    expect(normalizeError("boom")).toMatchObject({ code: "unknown", message: "boom" });
    expect(normalizeError(new Error("bad"))).toMatchObject({ code: "unknown", message: "bad" });
    expect(normalizeError(42)).toMatchObject({ code: "unknown" });
    expect(normalizeError(null)).toMatchObject({ code: "unknown" });
    expect(normalizeError({ code: 5 })).toMatchObject({ code: "unknown" });
  });

  it("fails gracefully outside Tauri", async () => {
    expect(isTauri()).toBe(false);
    await expect(getSettings()).rejects.toMatchObject({ code: "no_tauri" });
  });

  it("ui_event_exhaustiveness_helper covers every variant", () => {
    const samples: UiEvent[] = [
      { type: "chunk", text: "a" },
      { type: "toolCall", id: "1", title: "t", kind: "read", status: "pending" },
      { type: "toolUpdate", id: "1", status: "completed" },
      { type: "usage", promptTokens: 1, completionTokens: 2, tokensPerSecond: 3 },
      { type: "approval", id: 1, kind: "command", title: "t", command: "ls" },
      { type: "applyEdit", id: 2, rel: "a.txt", old: "o", newText: "x" },
      { type: "agentState", state: "running" },
      { type: "turnEnd", stopReason: "end_turn" },
      { type: "fsChanged", paths: [], overflow: false },
      { type: "gpu", usedMiB: 1, totalMiB: 2 },
      { type: "agentTerminalOutput", id: 7, chunk: "x" },
      { type: "agentTerminalTruncated", id: 7 },
      { type: "agentTerminalExit", id: 7, code: 0 },
      { type: "userTerminalExit", id: 1, code: 0 },
    ];
    const types = new Set(samples.map((s) => s.type));
    expect(types.size).toBe(samples.length);
    for (const s of samples) expect(describeEvent(s)).toContain(s.type);
  });

  it("command names match the Rust generate_handler! list", () => {
    const main = readFileSync(resolve(__dirname, "../../src-tauri/src/main.rs"), "utf8");
    const block = /generate_handler!\s*\[([\s\S]*?)\]/.exec(main);
    expect(block, "generate_handler! block").not.toBeNull();
    const rust = [...block![1].matchAll(/commands::(\w+)/g)].map((m) => m[1]).sort();
    expect(rust.length).toBeGreaterThan(10);
    expect([...COMMANDS].sort()).toEqual(rust);
  });
});
