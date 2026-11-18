import { BufferManager, type ModelHost, type ModelLike } from "../editor/models";
import { IpcError, type Stamp } from "../ipc";

/** Minimal fake of a Monaco text model: EOL aware, undo groups, alternative version ids. */
export class FakeModel implements ModelLike {
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

export function mkHost(files: Record<string, string>, defaultEol = "\r\n") {
  const stamps: Record<string, number> = {};
  const saves: { rel: string; text: string; stamp: Stamp | null }[] = [];
  const gates: Record<string, Promise<void> | undefined> = {};
  const saveGates: Record<string, Promise<void> | undefined> = {};
  const created: Record<string, FakeModel> = {};
  const stampOf = (rel: string): Stamp => ({ mtimeMs: stamps[rel] ?? 1, hash: String(files[rel]?.length) + ":" + (stamps[rel] ?? 1) });
  const host: ModelHost = {
    async readFile(rel) {
      // a gated read returns what the disk held when it STARTED (models a slow read)
      const gate = gates[rel];
      const startText = files[rel];
      const startStamp = rel in files ? stampOf(rel) : null;
      if (gate) await gate;
      if (gate && startText !== undefined && startStamp) return { text: startText, stamp: startStamp };
      if (!(rel in files)) throw new IpcError("not_found", rel);
      if (files[rel] === "\u0000binary") throw new IpcError("binary", rel);
      return { text: files[rel], stamp: stampOf(rel) };
    },
    async saveFile(rel, text, stamp) {
      saves.push({ rel, text, stamp });
      const sg = saveGates[rel];
      if (sg) await sg;
      if (stamp === null) {
        if (rel in files) throw new IpcError("conflict", "exists");
        files[rel] = text;
        stamps[rel] = (stamps[rel] ?? 1) + 1;
        return stampOf(rel);
      }
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
    host, files, saves, created, stamps, gates, saveGates,
    touch(rel: string, text: string) { files[rel] = text; stamps[rel] = (stamps[rel] ?? 1) + 1; },
  };
}
export const fm = (m: BufferManager, rel: string) => m.getModel(rel) as unknown as FakeModel;
