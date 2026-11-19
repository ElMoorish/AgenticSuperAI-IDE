/**
 * Crash recovery for unsaved editor text. Every `intervalMs` the dirty buffers are written to the
 * host's recovery store (only when their text changed); when a buffer is saved clean or closed its
 * snapshot is removed. After a crash `listRecovered` + `restoreBuffers` bring the text back.
 */
import type { BufferManager } from "./models";
import { normalizeError, recoveryClear, recoveryList, recoverySave, type RecoveredBuffer } from "../ipc";

export interface RecoveryDeps {
  models: Pick<BufferManager, "subscribe" | "getState" | "getBuffer">;
  save: (rel: string, text: string) => Promise<void>;
  clear: (rel: string) => Promise<void>;
}

const defaultSave = recoverySave;
const defaultClear = recoveryClear;

export class RecoverySync {
  private timer: ReturnType<typeof setInterval> | null = null;
  /** rel -> the text last written; presence means a snapshot exists on disk. */
  private written = new Map<string, string>();
  private busy = false;
  paused = false;

  constructor(private deps: RecoveryDeps, private intervalMs = 5000) {}

  start(): void {
    if (this.timer === null) this.timer = setInterval(() => void this.tick(), this.intervalMs);
  }
  stop(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
  }

  /** One pass. Never throws: recovery must not be able to disturb editing. */
  async tick(): Promise<void> {
    if (this.busy || this.paused) return;
    this.busy = true;
    try {
      const st = this.deps.models.getState();
      const dirty = new Set<string>();
      for (const rel of st.order) {
        const b = st.buffers[rel];
        if (!b?.dirty || b.status !== "ready") continue;
        dirty.add(rel);
        const text = this.deps.models.getBuffer(rel);
        if (text === undefined || this.written.get(rel) === text) continue;
        try {
          await this.deps.save(rel, text);
          this.written.set(rel, text);
        } catch (e) {
          if (normalizeError(e).code === "no_tauri") return; // plain browser: nothing to do
          console.error("recovery save failed", rel, normalizeError(e).message);
        }
      }
      for (const rel of [...this.written.keys()]) {
        if (dirty.has(rel)) continue;
        try {
          await this.deps.clear(rel);
        } catch {
          /* a stale snapshot is harmless */
        }
        this.written.delete(rel);
      }
    } finally {
      this.busy = false;
    }
  }

  /** The workspace changed or a restore finished: forget what we know about the disk state. */
  reset(): void {
    this.written.clear();
  }
}

export const defaultRecoveryDeps = (models: RecoveryDeps["models"]): RecoveryDeps => ({ models, save: defaultSave, clear: defaultClear });

export async function listRecovered(): Promise<RecoveredBuffer[]> {
  try {
    return await recoveryList();
  } catch {
    return [];
  }
}

/**
 * Open each file and put the recovered text into its buffer as an unsaved edit (the user decides
 * whether to save it). Skips files whose recovered text equals what is on disk. Returns the files restored.
 */
export async function restoreBuffers(
  models: Pick<BufferManager, "openFile" | "getBuffer" | "getBufferInfo" | "applyEdit">,
  items: RecoveredBuffer[],
  clear: (rel: string) => Promise<void> = recoveryClear,
): Promise<string[]> {
  const restored: string[] = [];
  for (const it of items) {
    try {
      await models.openFile(it.rel);
      const info = models.getBufferInfo(it.rel);
      if (!info || info.status !== "ready" || info.readOnly) continue;
      const cur = models.getBuffer(it.rel) ?? "";
      if (cur.replace(/\r\n/g, "\n") !== it.text.replace(/\r\n/g, "\n")) {
        models.applyEdit(it.rel, it.text);
        restored.push(it.rel);
      }
      await clear(it.rel).catch(() => {});
    } catch (e) {
      console.error("restore failed", it.rel, normalizeError(e).message);
    }
  }
  return restored;
}
