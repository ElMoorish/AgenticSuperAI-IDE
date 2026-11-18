/**
 * Editor buffers: one model per open file, dirty tracking, saves, external-change handling.
 *
 * The core (`BufferManager`) is framework-free and talks to a minimal `ModelLike` so Vitest can
 * use a fake; `monacoModels.ts` is the real Monaco adapter (lazy-loaded so the shell paints first).
 *
 * API for the agent flow (Tasks 10/11):
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
  saveFile(rel: string, text: string, stamp: Stamp): Promise<Stamp>;
  createModel(rel: string, text: string, language: string, readOnly: boolean): ModelLike | Promise<ModelLike>;
}

export type BufferStatus = "ready" | "oversize" | "binary" | "error";

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
  /** The file no longer exists on disk (unsaved text is kept). */
  deleted: boolean;
  canOpenAnyway: boolean;
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
const norm = (p: string): string => p.replace(/\\/g, "/").replace(/^\.?\//, "").replace(/\/+$/, "");

interface Entry {
  buf: Buffer;
  model?: ModelLike;
  sub?: { dispose(): void };
  savedAlt: number;
  eol: Eol;
  saving: boolean;
}

export class BufferManager {
  private entries = new Map<string, Entry>();
  private order: string[] = [];
  private active: string | null = null;
  private viewStates = new Map<string, unknown>();
  private pending = new Map<string, Promise<void>>();
  private listeners = new Set<() => void>();
  private snapshot: EditorState = { order: [], active: null, buffers: {} };
  /** The editor registers these so it can keep cursor/scroll across a silent reload. */
  hooks: { willReload?: (rel: string) => void; didReload?: (rel: string) => void } = {};

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

  // ---- reads ----
  getModel(rel: string): ModelLike | undefined {
    return this.entries.get(rel)?.model;
  }
  getBufferInfo(rel: string): Buffer | undefined {
    const e = this.entries.get(rel);
    return e ? { ...e.buf, viewState: this.viewStates.get(rel) } : undefined;
  }
  /** Live text including unsaved edits. */
  getBuffer(rel: string): string | undefined {
    return this.entries.get(rel)?.model?.getValue();
  }
  setViewState(rel: string, vs: unknown): void {
    this.viewStates.delete(rel);
    this.viewStates.set(rel, vs);
    while (this.viewStates.size > MAX_VIEW_STATES) this.viewStates.delete(this.viewStates.keys().next().value as string);
  }
  getViewState(rel: string): unknown {
    return this.viewStates.get(rel);
  }

  // ---- open / close ----
  activate(rel: string): void {
    if (this.entries.has(rel) && this.active !== rel) {
      this.active = rel;
      this.emit();
    }
  }

  openFile(rel: string, opts: { size?: number; anyway?: boolean } = {}): Promise<void> {
    rel = norm(rel);
    const cur = this.entries.get(rel);
    if (cur && (cur.buf.status === "ready" || ((cur.buf.status === "oversize" || cur.buf.status === "binary") && !opts.anyway))) {
      this.activate(rel);
      return Promise.resolve();
    }
    const inflight = this.pending.get(rel);
    if (inflight) return inflight.then(() => this.activate(rel));
    const p = this.load(rel, opts).finally(() => this.pending.delete(rel));
    this.pending.set(rel, p);
    return p;
  }

  /** "Open anyway (read-only)" for an over-limit file. */
  openAnyway(rel: string): Promise<void> {
    rel = norm(rel);
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
        conflict: null, deleted: false, canOpenAnyway: false, ...extra,
      },
      savedAlt: 0, eol: "\n", saving: false,
    });
    if (!this.order.includes(rel)) this.order.push(rel);
    this.active = rel;
    this.emit();
  }

  private async load(rel: string, opts: { size?: number; anyway?: boolean }): Promise<void> {
    if (!opts.anyway && opts.size !== undefined && opts.size > OPEN_LIMIT_BYTES) {
      this.placeholder(rel, "oversize", { size: opts.size, canOpenAnyway: true });
      return;
    }
    let res: ReadFileResult;
    try {
      res = await this.host.readFile(rel);
    } catch (e) {
      const code = errCode(e);
      if (code === "binary") this.placeholder(rel, "binary");
      else if (code === "too_large") this.placeholder(rel, "oversize", { canOpenAnyway: false, error: errMsg(e) });
      else this.placeholder(rel, "error", { error: errMsg(e) });
      return;
    }
    const size = byteLength(res.text);
    if (!opts.anyway && size > OPEN_LIMIT_BYTES) {
      this.placeholder(rel, "oversize", { size, canOpenAnyway: true });
      return;
    }
    const readOnly = opts.anyway === true; // "Open anyway" is always read-only
    const language = languageFor(rel);
    const model = await this.host.createModel(rel, res.text, language, readOnly);
    const eol = detectEol(res.text);
    if (model.getEOL() !== eol) model.setEOL(eol); // never let Monaco's platform default change the file's EOL
    this.dropEntry(rel);
    const entry: Entry = {
      buf: {
        rel, status: "ready", dirty: false, stamp: res.stamp, viewState: undefined, readOnly, language,
        conflict: null, deleted: false, canOpenAnyway: false, size,
      },
      model, savedAlt: model.getAlternativeVersionId(), eol, saving: false,
    };
    entry.sub = model.onDidChangeContent(() => this.refreshDirty(rel));
    this.entries.set(rel, entry);
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
  }

  /** Close a tab (the UI asks first when it is dirty). View state is kept for a later reopen. */
  closeFile(rel: string): void {
    if (!this.entries.has(rel)) return;
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
    this.emit();
  }

  // ---- edits ----
  /** Apply a full-text edit as ONE undo step. The file must already be open. */
  applyEdit(rel: string, newText: string): void {
    rel = norm(rel);
    const e = this.entries.get(rel);
    if (!e?.model) throw new Error(`${rel} is not open in the editor`);
    if (e.buf.readOnly) throw new Error(`${rel} is open read-only`);
    e.model.pushStackElement();
    e.model.replaceAll(newText);
    e.model.pushStackElement();
    this.refreshDirty(rel);
  }

  async ensureOpenAndApply(rel: string, newText: string): Promise<void> {
    rel = norm(rel);
    if (!this.entries.get(rel)?.model) await this.openFile(rel);
    this.applyEdit(rel, newText);
  }

  // ---- save ----
  async saveFile(rel: string): Promise<"saved" | "conflict"> {
    rel = norm(rel);
    const e = this.entries.get(rel);
    if (!e?.model || !e.buf.stamp) throw new Error(`${rel} is not open in the editor`);
    if (e.buf.readOnly) throw new Error(`${rel} is open read-only`);
    const model = e.model;
    const versionAtSave = model.getAlternativeVersionId();
    const text = model.getValue(); // carries the model's EOL (== the file's own EOL unless the user changed it)
    e.saving = true;
    try {
      const stamp = await this.host.saveFile(rel, text, e.buf.stamp);
      e.savedAlt = versionAtSave;
      this.patch(rel, { stamp, conflict: null, deleted: false, dirty: model.getAlternativeVersionId() !== versionAtSave });
      return "saved";
    } catch (err) {
      if (errCode(err) === "conflict") {
        this.patch(rel, { conflict: "disk" });
        return "conflict";
      }
      throw err;
    } finally {
      e.saving = false;
    }
  }

  // ---- conflict resolution ----
  /** Discard local edits and load the disk copy. */
  async reloadFromDisk(rel: string): Promise<void> {
    rel = norm(rel);
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
    rel = norm(rel);
    const e = this.entries.get(rel);
    if (!e?.model) throw new Error(`${rel} is not open in the editor`);
    const res = await this.host.readFile(rel);
    this.patch(rel, { stamp: res.stamp, conflict: null });
    return this.saveFile(rel);
  }

  /** Dismiss the banner and keep editing; the stamp stays stale so a save will prompt again. */
  keepMine(rel: string): void {
    this.patch(norm(rel), { conflict: null });
  }

  private applyDiskText(rel: string, e: Entry, res: ReadFileResult): void {
    const model = e.model!;
    this.hooks.willReload?.(rel);
    model.reload(res.text);
    const eol = detectEol(res.text);
    if (model.getEOL() !== eol) model.setEOL(eol);
    e.eol = eol;
    e.savedAlt = model.getAlternativeVersionId();
    e.buf = { ...e.buf, stamp: res.stamp, dirty: false, conflict: null, deleted: false };
    this.emit();
    this.hooks.didReload?.(rel);
  }

  // ---- external changes ----
  /** FsChanged: clean buffers reload silently; dirty ones get a conflict flag; deleted ones are marked. */
  async handleFsChanged(paths: string[], overflow: boolean): Promise<void> {
    const ps = paths.map(norm);
    const hits = [...this.entries.values()]
      .filter((e) => e.buf.status === "ready" && e.model)
      .map((e) => e.buf.rel)
      .filter((rel) => overflow || ps.some((p) => p === "" || p === rel || rel.startsWith(p + "/")));
    await Promise.all(hits.map((rel) => this.checkDisk(rel)));
  }

  private async checkDisk(rel: string): Promise<void> {
    const e = this.entries.get(rel);
    if (!e?.model || e.saving) return;
    let res: ReadFileResult;
    try {
      res = await this.host.readFile(rel);
    } catch (err) {
      if (errCode(err) === "not_found" && !e.buf.deleted) this.patch(rel, { deleted: true });
      return;
    }
    const cur = this.entries.get(rel);
    if (cur !== e || e.saving) return; // closed or saving while we were reading
    if (e.buf.stamp && res.stamp.hash === e.buf.stamp.hash && !e.buf.deleted) {
      e.buf = { ...e.buf, stamp: res.stamp }; // same content (e.g. our own save echo): nothing to do
      return;
    }
    if (!e.buf.dirty) {
      this.applyDiskText(rel, e, res);
    } else if (e.model.getValue() === res.text) {
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
