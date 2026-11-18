import { beforeEach, describe, expect, it } from "vitest";
import { BufferManager, detectEol, OPEN_LIMIT_BYTES, type ModelHost, type ModelLike } from "../editor/models";
import { IpcError, type Stamp } from "../ipc";

/** Minimal fake of a Monaco text model: EOL aware, undo groups, alternative version ids. */
class FakeModel implements ModelLike {
  text: string;
  eol: string;
  version = 1;
  alt = 1;
  undoStack: { before: string; beforeAlt: number }[] = [];
  closed = true;
  listeners = new Set<() => void>();
  disposed = false;
  constructor(text: string, defaultEol: string) {
    // Monaco picks the EOL from the text; with no line break it falls back to the platform default.
    this.eol = text.includes("\r\n") ? "\r\n" : text.includes("\n") ? "\n" : defaultEol;
    this.text = text.replace(/\r\n|\n/g, "\n");
  }
  getValue() { return this.text.replace(/\n/g, this.eol); }
  getEOL() { return this.eol; }
  setEOL(e: "\n" | "\r\n") { this.eol = e; this.bump(); }
  getAlternativeVersionId() { return this.alt; }
  pushStackElement() { this.closed = true; }
  replaceAll(t: string) {
    if (this.closed || this.undoStack.length === 0) this.undoStack.push({ before: this.text, beforeAlt: this.alt });
    this.closed = false;
    this.text = t.replace(/\r\n|\n/g, "\n");
    this.bump();
  }
  reload(t: string) {
    this.text = t.replace(/\r\n|\n/g, "\n");
    this.undoStack = [];
    this.bump();
  }
  undo() {
    const g = this.undoStack.pop();
    if (!g) return;
    this.text = g.before;
    this.alt = g.beforeAlt;
    this.closed = true;
    this.listeners.forEach((l) => l());
  }
  private bump() { this.version++; this.alt = this.version; this.listeners.forEach((l) => l()); }
  onDidChangeContent(cb: () => void) {
    this.listeners.add(cb);
    return { dispose: () => this.listeners.delete(cb) };
  }
  dispose() { this.disposed = true; }
}

function mkHost(files: Record<string, string>, defaultEol = "\r\n") {
  const stamps: Record<string, number> = {};
  const saves: { rel: string; text: string; stamp: Stamp }[] = [];
  const created: Record<string, FakeModel> = {};
  const stampOf = (rel: string): Stamp => ({ mtimeMs: stamps[rel] ?? 1, hash: String(files[rel]?.length) + ":" + (stamps[rel] ?? 1) });
  const host: ModelHost = {
    async readFile(rel) {
      if (!(rel in files)) throw new IpcError("not_found", rel);
      if (files[rel] === "\u0000binary") throw new IpcError("binary", rel);
      return { text: files[rel], stamp: stampOf(rel) };
    },
    async saveFile(rel, text, stamp) {
      saves.push({ rel, text, stamp });
      const cur = stampOf(rel);
      if (stamp.hash !== cur.hash) throw new IpcError("conflict", "changed");
      files[rel] = text;
      stamps[rel] = (stamps[rel] ?? 1) + 1;
      return stampOf(rel);
    },
    createModel(rel, text) {
      const m = new FakeModel(text, defaultEol);
      created[rel] = m;
      return m;
    },
  };
  return {
    host, files, saves, created, stamps,
    touch(rel: string, text: string) { files[rel] = text; stamps[rel] = (stamps[rel] ?? 1) + 1; },
  };
}
const fm = (m: BufferManager, rel: string) => m.getModel(rel) as unknown as FakeModel;

let h: ReturnType<typeof mkHost>;
let m: BufferManager;
beforeEach(() => {
  h = mkHost({ "a.txt": "one\ntwo\n", "crlf.txt": "a\r\nb\r\n", "one.txt": "single line", "big.bin": "x" });
  m = new BufferManager(h.host);
});

describe("models", () => {
  it("applyEdit_is_single_undo_step", async () => {
    await m.openFile("a.txt");
    const model = fm(m, "a.txt");
    model.replaceAll("one\ntwo\ntyped\n"); // user typing
    m.applyEdit("a.txt", "agent\nrewrite\n");
    model.replaceAll("agent\nrewrite\nmore typing\n");
    expect(model.undoStack).toHaveLength(3); // typing, agent edit, typing are separate steps
    model.undo();
    expect(model.getValue()).toBe("agent\nrewrite\n");
    model.undo();
    expect(model.getValue()).toBe("one\ntwo\ntyped\n"); // one undo reverts the whole agent edit
    expect(m.getBuffer("a.txt")).toBe("one\ntwo\ntyped\n");
  });

  it("applyEdit then undo returns to clean", async () => {
    await m.openFile("a.txt");
    m.applyEdit("a.txt", "changed\n");
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(true);
    fm(m, "a.txt").undo();
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(false);
  });

  it("applyEdit rejects clearly when the file is not open, ensureOpenAndApply opens it", async () => {
    expect(() => m.applyEdit("a.txt", "x")).toThrow(/not open/);
    await m.ensureOpenAndApply("a.txt", "x\n");
    expect(m.getBuffer("a.txt")).toBe("x\n");
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(true);
  });

  it("getBuffer returns unsaved text", async () => {
    expect(m.getBuffer("a.txt")).toBeUndefined();
    await m.openFile("a.txt");
    expect(m.getBuffer("a.txt")).toBe("one\ntwo\n");
    fm(m, "a.txt").replaceAll("edited\n");
    expect(m.getBuffer("a.txt")).toBe("edited\n");
    expect(h.files["a.txt"]).toBe("one\ntwo\n");
  });

  it("save_conflict_returns_conflict_and_keeps_dirty", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("mine\n");
    h.touch("a.txt", "theirs\n");
    expect(await m.saveFile("a.txt")).toBe("conflict");
    const info = m.getBufferInfo("a.txt")!;
    expect(info.dirty).toBe(true);
    expect(info.conflict).toBe("disk");
    expect(m.getBuffer("a.txt")).toBe("mine\n");
    expect(h.files["a.txt"]).toBe("theirs\n");
  });

  it("save success updates stamp and clears dirty", async () => {
    await m.openFile("a.txt");
    const before = m.getBufferInfo("a.txt")!.stamp;
    fm(m, "a.txt").replaceAll("new\n");
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(true);
    expect(await m.saveFile("a.txt")).toBe("saved");
    const info = m.getBufferInfo("a.txt")!;
    expect(info.dirty).toBe(false);
    expect(info.stamp).not.toEqual(before);
    expect(h.files["a.txt"]).toBe("new\n");
  });

  it("overwrite re-reads the stamp then saves; reload discards edits; keepMine dismisses", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("mine\n");
    h.touch("a.txt", "theirs\n");
    await m.saveFile("a.txt");
    expect(await m.overwrite("a.txt")).toBe("saved");
    expect(h.files["a.txt"]).toBe("mine\n");
    expect(m.getBufferInfo("a.txt")?.conflict).toBeNull();

    fm(m, "a.txt").replaceAll("mine2\n");
    h.touch("a.txt", "theirs2\n");
    await m.saveFile("a.txt");
    await m.reloadFromDisk("a.txt");
    expect(m.getBuffer("a.txt")).toBe("theirs2\n");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: false, conflict: null });

    fm(m, "a.txt").replaceAll("mine3\n");
    h.touch("a.txt", "theirs3\n");
    await m.saveFile("a.txt");
    m.keepMine("a.txt");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: true, conflict: null });
    expect(await m.saveFile("a.txt")).toBe("conflict"); // never silently overwrites
  });

  it("reopen_restores_view_state", async () => {
    await m.openFile("a.txt");
    const vs = { scrollTop: 120, cursor: [3, 4] };
    m.setViewState("a.txt", vs);
    m.closeFile("a.txt");
    expect(m.getBufferInfo("a.txt")).toBeUndefined();
    expect(fm(m, "a.txt")).toBeUndefined();
    await m.openFile("a.txt");
    expect(m.getBufferInfo("a.txt")?.viewState).toEqual(vs);
    expect(m.getViewState("a.txt")).toEqual(vs);
  });

  it("oversize_file_opens_read_only", async () => {
    await m.openFile("a.txt", { size: OPEN_LIMIT_BYTES + 1 });
    let info = m.getBufferInfo("a.txt")!;
    expect(info.status).toBe("oversize");
    expect(info.canOpenAnyway).toBe(true);
    expect(m.getModel("a.txt")).toBeUndefined();
    await m.openAnyway("a.txt");
    info = m.getBufferInfo("a.txt")!;
    expect(info.status).toBe("ready");
    expect(info.readOnly).toBe(true);
    expect(() => m.applyEdit("a.txt", "x")).toThrow(/read-only/);
    await expect(m.saveFile("a.txt")).rejects.toThrow(/read-only/);
  });

  it("oversize detected from text length when size is unknown; too_large is not openable", async () => {
    h.files["huge.txt"] = "x".repeat(OPEN_LIMIT_BYTES + 10);
    await m.openFile("huge.txt");
    expect(m.getBufferInfo("huge.txt")).toMatchObject({ status: "oversize", canOpenAnyway: true });
    h.host.readFile = async () => { throw new IpcError("too_large", "11 MB"); };
    await m.openFile("giant.txt");
    expect(m.getBufferInfo("giant.txt")).toMatchObject({ status: "oversize", canOpenAnyway: false });
  });

  it("crlf_text_roundtrips_unchanged", async () => {
    await m.openFile("crlf.txt");
    expect(fm(m, "crlf.txt").getEOL()).toBe("\r\n");
    expect(await m.saveFile("crlf.txt")).toBe("saved");
    expect(h.saves.at(-1)!.text).toBe("a\r\nb\r\n");
    fm(m, "crlf.txt").replaceAll("a\nb\nc\n"); // typed text arrives LF, model keeps CRLF
    await m.saveFile("crlf.txt");
    expect(h.saves.at(-1)!.text).toBe("a\r\nb\r\nc\r\n");
  });

  it("an LF file never becomes CRLF even when the platform default EOL is CRLF", async () => {
    await m.openFile("a.txt");
    expect(fm(m, "a.txt").getEOL()).toBe("\n");
    await m.openFile("one.txt"); // no line break at all: fake defaults to CRLF like Monaco on Windows
    expect(fm(m, "one.txt").getEOL()).toBe("\n");
    fm(m, "one.txt").replaceAll("single line!");
    await m.saveFile("one.txt");
    expect(h.saves.at(-1)!.text).toBe("single line!");
    expect(detectEol("a\r\nb\nc\r\n")).toBe("\r\n");
    expect(detectEol("a\nb\r\nc\n")).toBe("\n");
    expect(detectEol("")).toBe("\n");
  });

  it("external change reloads a clean buffer silently but never clobbers a dirty one", async () => {
    await m.openFile("a.txt");
    await m.openFile("crlf.txt");
    const seen: string[] = [];
    m.hooks.willReload = (r) => seen.push("will:" + r);
    m.hooks.didReload = (r) => seen.push("did:" + r);
    h.touch("a.txt", "disk v2\n");
    fm(m, "crlf.txt").replaceAll("typed\n");
    h.touch("crlf.txt", "disk crlf\r\n");
    await m.handleFsChanged(["a.txt", "crlf.txt"], false);
    expect(m.getBuffer("a.txt")).toBe("disk v2\n");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: false, conflict: null });
    expect(seen).toEqual(["will:a.txt", "did:a.txt"]);
    expect(m.getBuffer("crlf.txt")).toBe("typed\r\n");
    expect(m.getBufferInfo("crlf.txt")).toMatchObject({ dirty: true, conflict: "disk" });
  });

  it("our own save echo (same stamp) does not reload or flag a conflict", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("saved\n");
    await m.saveFile("a.txt");
    fm(m, "a.txt").replaceAll("typed after save\n");
    await m.handleFsChanged(["a.txt"], false);
    expect(m.getBuffer("a.txt")).toBe("typed after save\n");
    expect(m.getBufferInfo("a.txt")?.conflict).toBeNull();
  });

  it("overflow re-checks every open buffer; unrelated paths are ignored; folder deletion is honoured", async () => {
    h.files["d/x.txt"] = "x\n";
    await m.openFile("a.txt");
    await m.openFile("d/x.txt");
    h.touch("a.txt", "A2\n");
    await m.handleFsChanged(["other.txt"], false);
    expect(m.getBuffer("a.txt")).toBe("one\ntwo\n");
    await m.handleFsChanged([], true);
    expect(m.getBuffer("a.txt")).toBe("A2\n");
    delete h.files["d/x.txt"];
    await m.handleFsChanged(["d"], false);
    expect(m.getBufferInfo("d/x.txt")?.deleted).toBe(true);
  });

  it("deleted file marks the tab, keeps unsaved text, and recovers when it reappears", async () => {
    await m.openFile("a.txt");
    delete h.files["a.txt"];
    await m.handleFsChanged(["a.txt"], false);
    expect(m.getBufferInfo("a.txt")).toMatchObject({ deleted: true, status: "ready" });
    expect(m.getBuffer("a.txt")).toBe("one\ntwo\n");
    h.files["a.txt"] = "back\n";
    await m.handleFsChanged(["a.txt"], false);
    expect(m.getBufferInfo("a.txt")?.deleted).toBe(false);
    expect(m.getBuffer("a.txt")).toBe("back\n");
  });

  it("binary error code opens a binary placeholder with no model", async () => {
    h.files["x.png"] = "\u0000binary";
    await m.openFile("x.png");
    expect(m.getBufferInfo("x.png")).toMatchObject({ status: "binary" });
    expect(m.getModel("x.png")).toBeUndefined();
    expect(m.getBuffer("x.png")).toBeUndefined();
  });

  it("missing file reports an error state; open order and activation follow open/close", async () => {
    await m.openFile("nope.txt");
    expect(m.getBufferInfo("nope.txt")).toMatchObject({ status: "error" });
    await m.openFile("a.txt");
    await m.openFile("crlf.txt");
    expect(m.getState().active).toBe("crlf.txt");
    m.closeFile("crlf.txt");
    expect(m.getState().active).toBe("a.txt");
    expect(m.getState().order).toEqual(["nope.txt", "a.txt"]);
  });

  it("concurrent opens of the same file create one model", async () => {
    await Promise.all([m.openFile("a.txt"), m.openFile("a.txt")]);
    expect(m.getState().order).toEqual(["a.txt"]);
  });

  it("subscribers are notified on dirty changes and reset disposes models", async () => {
    await m.openFile("a.txt");
    let n = 0;
    const off = m.subscribe(() => n++);
    fm(m, "a.txt").replaceAll("z\n");
    expect(n).toBeGreaterThan(0);
    off();
    const model = fm(m, "a.txt");
    m.reset();
    expect(model.disposed).toBe(true);
    expect(m.getState().order).toEqual([]);
  });
});
