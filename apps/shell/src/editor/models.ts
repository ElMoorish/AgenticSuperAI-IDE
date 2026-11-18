/**
 * Editor buffers: one model per open file, dirty tracking, saves, external-change handling.
 *
 * The core (`BufferManager`) is framework-free and talks to a minimal `ModelLike` so Vitest can
 * use a fake; `monacoModels.ts` is the real Monaco adapter (lazy-loaded so the shell paints first).
 *
 * API for the agent flow (Tasks 10/11). Every method accepts any spelling of a path (backslashes,
 * "./", any case): identity is the case-folded canonical path.
 *  - `getBuffer(rel)`          live text incl. unsaved edits (undefined when the file is not open)
 *  - `applyEdit(rel, newText)` ONE undoable step (pushStackElement before and after); throws if the
 *                              file is not open / read-only. `ensureOpenAndApply` opens it first.
 *  - `saveFile(rel)`           "saved" | "conflict" (stamp-checked; dirty is kept on conflict)
 * Saves send the text with the file's own EOL (a CRLF file stays CRLF, an LF file stays LF).
 */
import { readFile, saveFile as ipcSaveFile, type ReadFileResult, type Stamp } from "../ipc";

/** Files over 5 MB are not opened unless the user chooses "Open anyway (read-only)". */
export const OPEN_LIMIT_BYTES = 5 * 1024 * 1024;
const MAX_VIEW_STATES = 100;

export type Eol = "\n" | "\r\n";

export interface ModelLike {
  getValue(): string;
  getEOL(): string;
  setEOL(eol: Eol): void;
  /** Monaco's alternative version id: returns to the saved value after an undo back to it. */
  getAlternativeVersionId(): number;
  pushStackElement(): void;
  /** Replace the whole content through pushEditOperations (undoable). */
  replaceAll(text: string): void;
  /** Replace the whole content and reset undo history (setValue). Used for reload from disk. */
  reload(text: string): void;
  onDidChangeContent(cb: () => void): { dispose(): void };
  dispose(): void;
}

export interface ModelHost {
  readFile(rel: string): Promise<ReadFileResult>;
  /** `stamp: null` = create-only (recreating a file deleted from disk). */
  saveFile(rel: string, text: string, stamp: Stamp | null): Promise<Stamp>;
  createModel(rel: string, text: string, language: string, readOnly: boolean): ModelLike | Promise<ModelLike>;
}

export type BufferStatus = "loading" | "ready" | "oversize" | "binary" | "error";

export interface Buffer {
  rel: string;
  status: BufferStatus;
  dirty: boolean;
  stamp: Stamp | null;
  viewState: unknown;
  readOnly: boolean;
  language: string;
  /** Set when the on-disk copy diverged from a dirty buffer (or a save was refused). */
  conflict: null | "disk";
  /** The file no longer exists on disk (unsaved text is kept; saving recreates it). */
  deleted: boolean;
  canOpenAnyway: boolean;
  /** The file mixes LF and CRLF; saving normalises it to one style (the UI warns). */
  mixedEol: boolean;
  size?: number;
  error?: string;
}

export interface EditorState {
  order: string[];
  active: string | null;
  buffers: Record<string, Buffer>;
}

export function detectEol(text: string): Eol {
  let crlf = 0;
  let lf = 0;
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) {
    if (i > 0 && text.charCodeAt(i - 1) === 13) crlf++;
    else lf++;
  }
  return crlf > lf ? "\r\n" : "\n";
}

const LANGS: Record<string, string> = {
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  json: "json", jsonc: "json", css: "css", scss: "scss", less: "less", html: "html", htm: "html",
  md: "markdown", rs: "rust", py: "python", go: "go", java: "java", c: "c", h: "c", cpp: "cpp", hpp: "cpp",
  cs: "csharp", rb: "ruby", php: "php", sh: "shell", bash: "shell", ps1: "powershell", bat: "bat", cmd: "bat",
  yml: "yaml", yaml: "yaml", toml: "ini", ini: "ini", xml: "xml", svg: "xml", sql: "sql", lua: "lua", kt: "kotlin",
  swift: "swift", dockerfile: "dockerfile",
};
export function languageFor(rel: string): string {
  const name = rel.slice(rel.lastIndexOf("/") + 1).toLowerCase();
  if (name === "dockerfile") return "dockerfile";
  const dot = name.lastIndexOf(".");
  return dot > 0 ? (LANGS[name.slice(dot + 1)] ?? "plaintext") : "plaintext";
}

const errCode = (e: unknown): string => (e && typeof e === "object" && typeof (e as { code?: unknown }).code === "string" ? (e as { code: string }).code : "unknown");
const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));
const byteLength = (s: string): number => (s.length * 3 <= OPEN_LIMIT_BYTES ? s.length : new TextEncoder().encode(s).length);
/** Canonical workspace-relative form: forward slashes, no empty or "." segments. */
const norm = (p: string): string =>
  p.replace(/\\/g, "/").split("/").filter((seg) => seg !== "" && seg !== ".").join("/");
const foldKey = (p: string): string => p.toLowerCase(); // the host filesystem (NTFS) is case-insensitive
const hasMixedEol = (text: string): boolean => {
  const lf = text.split("\n").length - 1;
  const crlf = text.split("\r\n").length - 1;
  return crlf > 0 && lf > crlf;
};
const sameText = (a: string, b: string): boolean => a === b || a.replace(/\r\n/g, "\n") === b.replace(/\r\n/g, "\n");

interface Entry {
  buf: Buffer;
  model?: ModelLike;
  sub?: { dispose(): void };
  savedAlt: number;
  eol: Eol;
  saving: boolean;
  /** In-flight save, so a second Ctrl+S joins it instead of sending a stale stamp. */
  savePromise?: Promise<"saved" | "conflict">;
  /** Bumped on every save/reload; a disk read that started under an older value is stale. */
  gen: number;
  /** An FsChanged arrived while saving; look at the disk again once the save is done. */
  recheck: boolean;
}

export class BufferManager {
  private entries = new Map<string, Entry>();
  private order: string[] = [];
  private active: string | null = null;
  private viewStates = new Map<string, unknown>();
  private pending = new Map<string, Promise<void>>();
  /** Current load token per rel: closing the tab or resetting invalidates an in-flight load. */
  private loads = new Map<string, object>();
  /** folded path -> the rel an entry or load is registered under (case-insensitive identity). */
  private names = new Map<string, string>();
  private listeners = new Set<() => void>();
  private snapshot: EditorState = { order: [], active: null, buffers: {} };
  /** The editor registers these so it can keep cursor/scroll across reloads and closes. */
  hooks: {
    willReload?: (rel: string) => void;
    didReload?: (rel: string) => void;
    willClose?: (rel: string) => void;
  } = {};

  constructor(private host: ModelHost) {}

  // ---- store plumbing ----
  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  };
  getState = (): EditorState => this.snapshot;
  private emit(): void {
    const buffers: Record<string, Buffer> = {};
    for (const [rel, e] of this.entries) buffers[rel] = e.buf;
    this.snapshot = { order: [...this.order], active: this.active, buffers };
    for (const l of [...this.listeners]) l();
  }
  private patch(rel: string, p: Partial<Buffer>): void {
    const e = this.entries.get(rel);
    if (!e) return;
    e.buf = { ...e.buf, ...p };
    this.emit();
  }
  private refreshDirty(rel: string): void {
    const e = this.entries.get(rel);
    if (!e?.model) return;
    const dirty = e.model.getAlternativeVersionId() !== e.savedAlt;
    if (dirty !== e.buf.dirty) this.patch(rel, { dirty });
  }

  /** Map any spelling of a path (slashes, "./", case) to the rel its buffer is registered under. */
  resolve(p: string): string {
    const n = norm(p);
    return this.names.get(foldKey(n)) ?? n;
  }

  // ---- reads ----
  getModel(rel: string): ModelLike | undefined {
    return this.entries.get(this.resolve(rel))?.model;
  }
  getBufferInfo(rel: string): Buffer | undefined {
    rel = this.resolve(rel);
    const e = this.entries.get(rel);
    return e ? { ...e.buf, viewState: this.viewStates.get(rel) } : undefined;
  }
  /** Live text including unsaved edits. Accepts any spelling of the path. */
  getBuffer(rel: string): string | undefined {
    return this.entries.get(this.resolve(rel))?.model?.getValue();
  }
  setViewState(rel: string, vs: unknown): void {
    rel = this.resolve(rel);
    this.viewStates.delete(rel);
    this.viewStates.set(rel, vs);
    while (this.viewStates.size > MAX_VIEW_STATES) this.viewStates.delete(this.viewStates.keys().next().value as string);
  }
  getViewState(rel: string): unknown {
    return this.viewStates.get(this.resolve(rel));
  }

  // ---- open / close ----
  activate(rel: string): void {
    rel = this.resolve(rel);
    if (this.entries.has(rel) && this.active !== rel) {
      this.active = rel;
      this.emit();
    }
  }

  openFile(rel: string, opts: { size?: number; anyway?: boolean } = {}): Promise<void> {
    rel = this.resolve(rel);
    const cur = this.entries.get(rel);
    if (cur && (cur.buf.status === "ready" || ((cur.buf.status === "oversize" || cur.buf.status === "binary") && !opts.anyway))) {
      this.activate(rel);
      return Promise.resolve();
    }
    const inflight = this.pending.get(rel);
    if (inflight) return inflight.then(() => this.activate(rel));
    const tok = {};
    this.loads.set(rel, tok);
    this.names.set(foldKey(rel), rel);
    const p: Promise<void> = this.load(rel, opts, tok).finally(() => {
      if (this.pending.get(rel) === p) this.pending.delete(rel);
      if (this.loads.get(rel) === tok) this.loads.delete(rel);
      if (!this.entries.has(rel) && !this.loads.has(rel)) this.names.delete(foldKey(rel));
    });
    this.pending.set(rel, p);
    return p;
  }

  /** "Open anyway (read-only)" for an over-limit file. */
  openAnyway(rel: string): Promise<void> {
    rel = this.resolve(rel);
    const cur = this.entries.get(rel);
    if (cur && cur.buf.status === "oversize" && !cur.buf.canOpenAnyway) return Promise.resolve();
    this.dropEntry(rel);
    return this.openFile(rel, { anyway: true });
  }

  private placeholder(rel: string, status: BufferStatus, extra: Partial<Buffer> = {}): void {
    this.dropEntry(rel);
    this.entries.set(rel, {
      buf: {
        rel, status, dirty: false, stamp: null, viewState: undefined, readOnly: false, language: languageFor(rel),
        conflict: null, deleted: false, canOpenAnyway: false, mixedEol: false, ...extra,
      },
      savedAlt: 0, eol: "\n", saving: false, gen: 0, recheck: false,
    });
    this.names.set(foldKey(rel), rel);
    if (!this.order.includes(rel)) this.order.push(rel);
    this.active = rel;
    this.emit();
  }

  private async load(rel: string, opts: { size?: number; anyway?: boolean }, tok: object): Promise<void> {
    const live = (): boolean => this.loads.get(rel) === tok;
    if (!opts.anyway && opts.size !== undefined && opts.size > OPEN_LIMIT_BYTES) {
      this.placeholder(rel, "oversize", { size: opts.size, canOpenAnyway: true });
      return;
    }
    // Show the tab right away so a slow read or the lazy Monaco chunk never looks like a dead click.
    if (!this.entries.has(rel)) this.placeholder(rel, "loading");
    let res: ReadFileResult;
    try {
      res = await this.host.readFile(rel);
    } catch (e) {
      if (!live()) return;
      const code = errCode(e);
      if (code === "binary") this.placeholder(rel, "binary");
      else if (code === "too_large") this.placeholder(rel, "oversize", { canOpenAnyway: false, error: errMsg(e) });
      else this.placeholder(rel, "error", { error: errMsg(e) });
      return;
    }
    if (!live()) return;
    const size = byteLength(res.text);
    if (!opts.anyway && size > OPEN_LIMIT_BYTES) {
      this.placeholder(rel, "oversize", { size, canOpenAnyway: true });
      return;
    }
    const readOnly = opts.anyway === true; // "Open anyway" is always read-only
    const language = languageFor(rel);
    let model: ModelLike;
    try {
      model = await this.host.createModel(rel, res.text, language, readOnly);
    } catch (e) {
      if (live()) this.placeholder(rel, "error", { error: `The editor could not be loaded: ${errMsg(e)}` });
      return;
    }
    if (!live()) {
      model.dispose(); // closed, or the workspace was reset, while we were loading
      return;
    }
    const eol = detectEol(res.text);
    if (model.getEOL() !== eol) model.setEOL(eol); // never let Monaco's platform default change the file's EOL
    this.dropEntry(rel);
    const entry: Entry = {
      buf: {
        rel, status: "ready", dirty: false, stamp: res.stamp, viewState: undefined, readOnly, language,
        conflict: null, deleted: false, canOpenAnyway: false, mixedEol: hasMixedEol(res.text), size,
      },
      model, savedAlt: model.getAlternativeVersionId(), eol, saving: false, gen: 0, recheck: false,
    };
    entry.sub = model.onDidChangeContent(() => this.refreshDirty(rel));
    this.entries.set(rel, entry);
    this.names.set(foldKey(rel), rel);
    if (!this.order.includes(rel)) this.order.push(rel);
    this.active = rel;
    this.emit();
  }

  private dropEntry(rel: string): void {
    const e = this.entries.get(rel);
    if (!e) return;
    e.sub?.dispose();
    e.model?.dispose();
    this.entries.delete(rel);
    if (!this.loads.has(rel)) this.names.delete(foldKey(rel));
  }

  /** Close a tab (the UI asks first when it is dirty). View state is kept for a later reopen. */
  closeFile(rel: string): void {
    rel = this.resolve(rel);
    this.loads.delete(rel); // cancels an in-flight load (it disposes its model when it lands)
    this.pending.delete(rel);
    if (!this.entries.has(rel)) {
      this.names.delete(foldKey(rel));
      return;
    }
    this.hooks.willClose?.(rel); // lets the editor save the view state before the model goes away
    const idx = this.order.indexOf(rel);
    this.dropEntry(rel);
    this.order = this.order.filter((r) => r !== rel);
    if (this.active === rel) this.active = this.order[Math.min(idx, this.order.length - 1)] ?? null;
    this.emit();
  }

  /** Dispose everything (workspace switch). */
  reset(): void {
    for (const rel of [...this.entries.keys()]) this.dropEntry(rel);
    this.order = [];
    this.active = null;
    this.viewStates.clear();
    this.pending.clear();
    this.loads.clear(); // in-flight loads of the old workspace must not land in the new one
    this.names.clear();
    this.emit();
  }

  // ---- edits ----
  /** Apply a full-text edit as ONE undo step. The file must already be open. */
  applyEdit(rel: string, newText: string): void {
    rel = this.resolve(rel);
    const e = this.entries.get(rel);
    if (!e?.model) throw new Error(`${rel} is not open in the editor`);
    if (e.buf.readOnly) throw new Error(`${rel} is open read-only`);
    e.model.pushStackElement();
    e.model.replaceAll(newText);
    e.model.pushStackElement();
    this.refreshDirty(rel);
  }

  async ensureOpenAndApply(rel: string, newText: string): Promise<void> {
    rel = this.resolve(rel);
    if (!this.entries.get(rel)?.model) await this.openFile(rel);
    this.applyEdit(this.resolve(rel), newText);
  }

  // ---- save ----
  async saveFile(rel: string): Promise<"saved" | "conflict"> {
    rel = this.resolve(rel);
    const e = this.entries.get(rel);
    if (!e?.model || e.buf.status !== "ready") throw new Error(`${rel} is not open in the editor`);
    if (e.buf.readOnly) throw new Error(`${rel} is open read-only`);
    // A second Ctrl+S while one is in flight joins it (a stale stamp would raise a false conflict).
    if (e.savePromise) {
      await e.savePromise.catch(() => undefined);
      if (this.entries.get(rel) !== e) return "saved";
    }
    // Nothing to write: leave the file alone (this also keeps a mixed-EOL file byte-identical).
    if (!e.buf.dirty && !e.buf.deleted && !e.buf.conflict) return "saved";
    const run = this.doSave(rel, e);
    e.savePromise = run;
    try {
      return await run;
    } finally {
      if (e.savePromise === run) e.savePromise = undefined;
    }
  }

  private async doSave(rel: string, e: Entry): Promise<"saved" | "conflict"> {
    const model = e.model!;
    const versionAtSave = model.getAlternativeVersionId();
    const text = model.getValue(); // carries the model's EOL (== the file's own EOL unless the user changed it)
    e.saving = true;
    try {
      // A deleted file is recreated create-only (stamp null): that conflicts if something reappeared.
      const stamp = await this.host.saveFile(rel, text, e.buf.deleted ? null : e.buf.stamp);
      e.savedAlt = versionAtSave;
      e.gen++;
      this.patch(rel, {
        stamp, conflict: null, deleted: false, mixedEol: false,
        dirty: model.getAlternativeVersionId() !== versionAtSave,
      });
      return "saved";
    } catch (err) {
      if (errCode(err) === "conflict") {
        this.patch(rel, { conflict: "disk" });
        return "conflict";
      }
      throw err;
    } finally {
      e.saving = false;
      if (e.recheck) {
        e.recheck = false;
        void this.checkDisk(rel);
      }
    }
  }

  // ---- conflict resolution ----
  /** Discard local edits and load the disk copy. */
  async reloadFromDisk(rel: string): Promise<void> {
    rel = this.resolve(rel);
    const e = this.entries.get(rel);
    if (!e?.model) return;
    let res: ReadFileResult;
    try {
      res = await this.host.readFile(rel);
    } catch (err) {
      if (errCode(err) === "not_found") this.patch(rel, { deleted: true });
      else this.patch(rel, { error: errMsg(err) });
      return;
    }
    this.applyDiskText(rel, e, res);
  }

  /** Re-read the current disk stamp, then save over it (explicit user choice). */
  async overwrite(rel: string): Promise<"saved" | "conflict"> {
    rel = this.resolve(rel);
    const e = this.entries.get(rel);
    if (!e?.model) throw new Error(`${rel} is not open in the editor`);
    try {
      const res = await this.host.readFile(rel);
      this.patch(rel, { stamp: res.stamp, deleted: false });
    } catch (err) {
      if (errCode(err) !== "not_found") throw err;
      this.patch(rel, { deleted: true }); // gone: recreate it create-only
    }
    // The user chose to overwrite: write even when the buffer looks clean.
    if (!e.buf.dirty && !e.buf.deleted) this.patch(rel, { conflict: "disk" });
    return this.saveFile(rel);
  }

  /** Dismiss the banner and keep editing; the stamp stays stale so a save will prompt again. */
  keepMine(rel: string): void {
    this.patch(this.resolve(rel), { conflict: null });
  }

  private applyDiskText(rel: string, e: Entry, res: ReadFileResult): void {
    const model = e.model!;
    this.hooks.willReload?.(rel);
    model.reload(res.text);
    const eol = detectEol(res.text);
    if (model.getEOL() !== eol) model.setEOL(eol);
    e.eol = eol;
    e.savedAlt = model.getAlternativeVersionId();
    e.gen++;
    e.buf = { ...e.buf, stamp: res.stamp, dirty: false, conflict: null, deleted: false, mixedEol: hasMixedEol(res.text) };
    this.emit();
    this.hooks.didReload?.(rel);
  }

  // ---- external changes ----
  /** FsChanged: clean buffers reload silently; dirty ones get a conflict flag; deleted ones are marked. */
  async handleFsChanged(paths: string[], overflow: boolean): Promise<void> {
    const ps = paths.map((p) => foldKey(norm(p)));
    const hits = [...this.entries.values()]
      .filter((e) => e.buf.status === "ready" && e.model)
      .map((e) => e.buf.rel)
      .filter((rel) => {
        const k = foldKey(rel);
        return overflow || ps.some((p) => p === "" || p === k || k.startsWith(p + "/"));
      });
    await Promise.all(hits.map((rel) => this.checkDisk(rel).catch(() => undefined)));
  }

  private async checkDisk(rel: string): Promise<void> {
    const e = this.entries.get(rel);
    if (!e?.model) return;
    if (e.saving) {
      e.recheck = true; // looked at again as soon as the save finishes
      return;
    }
    const gen = e.gen;
    let res: ReadFileResult;
    try {
      res = await this.host.readFile(rel);
    } catch (err) {
      const fresh = this.entries.get(rel) === e && e.gen === gen && !e.saving;
      if (errCode(err) === "not_found" && fresh && !e.buf.deleted) this.patch(rel, { deleted: true });
      return;
    }
    if (this.entries.get(rel) !== e) return; // closed while we were reading
    if (e.saving || e.gen !== gen) {
      // A save or reload landed while we were reading: this read may be older than our state.
      if (e.saving) e.recheck = true;
      else void this.checkDisk(rel);
      return;
    }
    if (e.buf.stamp && res.stamp.hash === e.buf.stamp.hash && !e.buf.deleted) {
      e.buf = { ...e.buf, stamp: res.stamp }; // same content (e.g. our own save echo): nothing to do
      return;
    }
    if (!e.buf.dirty) {
      this.applyDiskText(rel, e, res);
    } else if (sameText(e.model.getValue(), res.text)) {
      e.buf = { ...e.buf, stamp: res.stamp, conflict: null, deleted: false };
      this.emit();
    } else {
      this.patch(rel, { conflict: "disk", deleted: false });
    }
  }
}

// ---- app singleton wired to ipc + Monaco (lazy) ----
const realHost: ModelHost = {
  readFile,
  saveFile: ipcSaveFile,
  createModel: async (rel, text, language, readOnly) => {
    const { createMonacoModel } = await import("./monacoModels");
    return createMonacoModel(rel, text, language, readOnly);
  },
};

export const models = new BufferManager(realHost);
export const openFile = (rel: string, opts?: { size?: number; anyway?: boolean }) => models.openFile(rel, opts);
export const getBuffer = (rel: string) => models.getBuffer(rel);
export const applyEdit = (rel: string, newText: string) => models.applyEdit(rel, newText);
export const ensureOpenAndApply = (rel: string, newText: string) => models.ensureOpenAndApply(rel, newText);
export const saveFile = (rel: string) => models.saveFile(rel);
