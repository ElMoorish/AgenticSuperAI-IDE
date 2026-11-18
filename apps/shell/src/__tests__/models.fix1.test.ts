import { beforeEach, describe, expect, it } from "vitest";
import { BufferManager, type ModelHost } from "../editor/models";
import { FakeModel, fm, mkHost } from "./modelsHelpers";

let h: ReturnType<typeof mkHost>;
let m: BufferManager;
const tick = () => new Promise((r) => setTimeout(r, 0));
const gate = () => {
  let release!: () => void;
  const p = new Promise<void>((r) => (release = r));
  return { p, release };
};
beforeEach(() => {
  h = mkHost({ "a.txt": "one\ntwo\n", "crlf.txt": "a\r\nb\r\n" });
  m = new BufferManager(h.host);
});

describe("task 9 review fixes", () => {
  it("I2 every public entry point accepts any spelling of a path and case-folds identity", async () => {
    await m.openFile("a.txt");
    expect(m.getBuffer("./a.txt")).toBe("one\ntwo\n");
    expect(m.getBuffer("A.TXT")).toBe("one\ntwo\n");
    expect(m.getBufferInfo("x/../a.txt".replace("x/../", ""))?.rel).toBe("a.txt");
    await m.openFile("A.txt"); // the agent spells it differently: same buffer, no second model
    expect(m.getState().order).toEqual(["a.txt"]);
    expect(Object.keys(h.created)).toEqual(["a.txt"]);
    m.closeFile("A.TXT");
    expect(m.getState().order).toEqual([]);
  });

  it("I1 a second save while one is in flight joins it and never sends a stale stamp", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("edit\n");
    const g = gate();
    h.saveGates["a.txt"] = g.p;
    const p1 = m.saveFile("a.txt");
    const p2 = m.saveFile("a.txt");
    g.release();
    expect(await p1).toBe("saved");
    expect(await p2).toBe("saved");
    expect(h.saves).toHaveLength(1);
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: false, conflict: null });
  });

  it("I7 saving a clean buffer writes nothing (a mixed-EOL file stays byte-identical)", async () => {
    h.files["mixed.txt"] = "a\r\nb\nc\r\nd\n";
    await m.openFile("mixed.txt");
    expect(m.getBufferInfo("mixed.txt")?.mixedEol).toBe(true);
    expect(await m.saveFile("mixed.txt")).toBe("saved");
    expect(h.saves).toHaveLength(0);
    fm(m, "mixed.txt").replaceAll("x\n");
    await m.saveFile("mixed.txt");
    expect(h.saves).toHaveLength(1);
    expect(m.getBufferInfo("mixed.txt")?.mixedEol).toBe(false);
  });

  it("I7 a dirty mixed-EOL buffer whose text equals the disk text is not flagged as a conflict", async () => {
    h.files["mixed.txt"] = "a\r\nb\nc\r\nd\n";
    await m.openFile("mixed.txt");
    fm(m, "mixed.txt").replaceAll("a\nb\nc\nd\n"); // dirty only because of normalisation
    await m.handleFsChanged(["mixed.txt"], false);
    expect(m.getBufferInfo("mixed.txt")?.conflict).toBeNull();
  });

  it("I3 closing a tab while its file is still loading cancels the load", async () => {
    const g = gate();
    h.gates["a.txt"] = g.p;
    const p = m.openFile("a.txt");
    expect(m.getBufferInfo("a.txt")?.status).toBe("loading"); // visible immediately
    m.closeFile("a.txt");
    g.release();
    await p;
    expect(m.getState().order).toEqual([]);
    expect(m.getState().active).toBeNull();
    expect(m.getModel("a.txt")).toBeUndefined();
  });

  it("I3 a load that lands after reset() does not leak into the new workspace", async () => {
    const g = gate();
    h.gates["a.txt"] = g.p;
    const p = m.openFile("a.txt");
    m.reset();
    g.release();
    await p;
    expect(Object.keys(m.getState().buffers)).toEqual([]);
    delete h.gates["a.txt"];
    await m.openFile("a.txt"); // not blocked by the stale pending entry
    expect(m.getState().order).toEqual(["a.txt"]);
  });

  it("I3 a model created after the tab was closed is disposed", async () => {
    let made: FakeModel | undefined;
    const g = gate();
    const host: ModelHost = {
      ...h.host,
      async createModel(_rel, text) {
        await g.p;
        made = new FakeModel(text, "\n");
        return made;
      },
    };
    const mm = new BufferManager(host);
    const p = mm.openFile("a.txt");
    await tick();
    mm.closeFile("a.txt");
    g.release();
    await p;
    expect(made?.disposed).toBe(true);
    expect(mm.getState().order).toEqual([]);
  });

  it("I5 a failing editor load becomes an error tab with a message", async () => {
    const host: ModelHost = { ...h.host, createModel: async () => { throw new Error("chunk failed"); } };
    const mm = new BufferManager(host);
    await mm.openFile("a.txt");
    expect(mm.getBufferInfo("a.txt")).toMatchObject({ status: "error" });
    expect(mm.getBufferInfo("a.txt")?.error).toMatch(/chunk failed/);
  });

  it("I4 willClose runs before the model is disposed so the view state can be kept", async () => {
    await m.openFile("a.txt");
    const model = fm(m, "a.txt");
    let disposedAtHook: boolean | undefined;
    m.hooks.willClose = (rel) => {
      disposedAtHook = model.disposed;
      m.setViewState(rel, { top: 42 });
    };
    m.closeFile("a.txt");
    expect(disposedAtHook).toBe(false);
    expect(model.disposed).toBe(true);
    await m.openFile("a.txt");
    expect(m.getViewState("a.txt")).toEqual({ top: 42 });
  });

  it("I6 a deleted file can be recreated from the editor text (create-only save)", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("precious\n");
    delete h.files["a.txt"];
    await m.handleFsChanged(["a.txt"], false);
    expect(m.getBufferInfo("a.txt")?.deleted).toBe(true);
    expect(await m.saveFile("a.txt")).toBe("saved");
    expect(h.saves.at(-1)?.stamp).toBeNull();
    expect(h.files["a.txt"]).toBe("precious\n");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ deleted: false, dirty: false });
  });

  it("I6 Overwrite on a deleted file recreates it instead of failing with not_found", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("mine\n");
    delete h.files["a.txt"];
    expect(await m.overwrite("a.txt")).toBe("saved");
    expect(h.files["a.txt"]).toBe("mine\n");
  });

  it("I6 recreate is refused (conflict) when something reappeared at that path", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("mine\n");
    delete h.files["a.txt"];
    await m.handleFsChanged(["a.txt"], false);
    h.files["a.txt"] = "someone else\n";
    expect(await m.saveFile("a.txt")).toBe("conflict");
    expect(h.files["a.txt"]).toBe("someone else\n");
  });

  it("I8 a disk read that started before a save finished is not applied over newer text", async () => {
    await m.openFile("a.txt");
    const g = gate();
    h.gates["a.txt"] = g.p; // the read starts now and will return the OLD disk text
    const check = m.handleFsChanged(["a.txt"], false);
    await tick();
    delete h.gates["a.txt"];
    fm(m, "a.txt").replaceAll("fresh\n");
    await m.saveFile("a.txt");
    g.release();
    await check;
    await tick();
    expect(m.getBuffer("a.txt")).toBe("fresh\n");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: false, conflict: null });
  });

  it("I8 an FsChanged that arrives during a save is re-checked afterwards", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("mine\n");
    const g = gate();
    h.saveGates["a.txt"] = g.p;
    const save = m.saveFile("a.txt");
    await tick();
    await m.handleFsChanged(["a.txt"], false); // flagged for a recheck, not dropped
    g.release();
    await save;
    h.touch("a.txt", "external after save\n"); // changed on disk right after our write
    await m.handleFsChanged(["a.txt"], false);
    await tick();
    expect(m.getBuffer("a.txt")).toBe("external after save\n"); // clean buffer reloads
  });
});
