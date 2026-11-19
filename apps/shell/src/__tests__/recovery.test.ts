import { describe, expect, it, vi } from "vitest";
import { RecoverySync, restoreBuffers } from "../editor/recovery";

type B = { dirty: boolean; status: string };
function fakeModels(init: Record<string, { text: string; dirty: boolean }>) {
  const files = { ...init };
  return {
    files,
    models: {
      subscribe: () => () => {},
      getState: () => ({
        order: Object.keys(files),
        active: null,
        buffers: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { dirty: v.dirty, status: "ready" } as B])),
      }),
      getBuffer: (r: string) => files[r]?.text,
    },
  };
}

describe("RecoverySync", () => {
  it("writes dirty buffers once per change and clears snapshots when they become clean or close", async () => {
    const f = fakeModels({ "a.txt": { text: "one", dirty: true }, "b.txt": { text: "clean", dirty: false } });
    const save = vi.fn().mockResolvedValue(undefined);
    const clear = vi.fn().mockResolvedValue(undefined);
    const sync = new RecoverySync({ models: f.models as never, save, clear });
    await sync.tick();
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith("a.txt", "one");
    await sync.tick(); // unchanged: no rewrite
    expect(save).toHaveBeenCalledTimes(1);
    f.files["a.txt"].text = "two";
    await sync.tick();
    expect(save).toHaveBeenLastCalledWith("a.txt", "two");
    f.files["a.txt"].dirty = false; // saved
    await sync.tick();
    expect(clear).toHaveBeenCalledWith("a.txt");
    delete f.files["a.txt"];
    await sync.tick();
    expect(clear).toHaveBeenCalledTimes(1);
  });

  it("a failing save does not throw and is retried next tick; paused does nothing", async () => {
    const f = fakeModels({ "a.txt": { text: "x", dirty: true } });
    const save = vi.fn().mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(undefined);
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const sync = new RecoverySync({ models: f.models as never, save, clear: vi.fn() });
    sync.paused = true;
    await sync.tick();
    expect(save).not.toHaveBeenCalled();
    sync.paused = false;
    await sync.tick();
    await sync.tick();
    expect(save).toHaveBeenCalledTimes(2);
    spy.mockRestore();
  });
});

describe("restoreBuffers", () => {
  it("puts recovered text into the buffer as an unsaved edit and skips identical text", async () => {
    const text: Record<string, string> = { "a.txt": "disk\n", "same.txt": "same\n" };
    const m = {
      openFile: vi.fn(async () => {}),
      getBufferInfo: () => ({ status: "ready", readOnly: false }),
      getBuffer: (r: string) => text[r],
      applyEdit: vi.fn((r: string, t: string) => { text[r] = t; }),
    };
    const clear = vi.fn().mockResolvedValue(undefined);
    const out = await restoreBuffers(m as never, [
      { rel: "a.txt", text: "mine\n", savedAtMs: 1 },
      { rel: "same.txt", text: "same\r\n", savedAtMs: 2 },
    ], clear);
    expect(out).toEqual(["a.txt"]);
    expect(m.applyEdit).toHaveBeenCalledTimes(1);
    expect(clear).toHaveBeenCalledTimes(2);
  });
});
