/**
 * Applies an accepted agent edit (`applyEdit` UiEvent) to the editor buffer, so it lands as ONE
 * undoable step and goes through the normal stamp-checked save, then tells the host (`confirm_edit`)
 * so the host can answer the agent's pending `fs/write_text_file`.
 *
 * It never overwrites user work: a buffer with unsaved edits, a read-only/conflicted buffer, or a
 * buffer that no longer matches the text the agent based its proposal on all fail the edit with a
 * message the agent receives. If the save fails after the buffer was changed, the buffer is
 * reloaded from disk so no half-applied agent text is left behind.
 */
import { models as appModels, type BufferManager } from "../editor/models";
import { confirmEdit as ipcConfirm, normalizeError, type UiEvent } from "../ipc";

type ApplyEditEvent = Extract<UiEvent, { type: "applyEdit" }>;

export interface ApplyDeps {
  models: Pick<BufferManager, "getBufferInfo" | "getBuffer" | "openFile" | "applyEdit" | "saveFile" | "reloadFromDisk">;
  confirm: (id: number, ok: boolean, error?: string) => Promise<void>;
}

const defaultDeps: ApplyDeps = { models: appModels, confirm: ipcConfirm };
const eolFree = (t: string): string => t.replace(/\r\n/g, "\n");

const inFlight = new Set<number>();
let queue: Promise<unknown> = Promise.resolve();

/** Serialised (one edit at a time) and idempotent per approval id (the host may replay one). */
export function handleApplyEdit(ev: ApplyEditEvent, deps: ApplyDeps = defaultDeps): Promise<void> {
  if (inFlight.has(ev.id)) return Promise.resolve();
  inFlight.add(ev.id);
  const run = queue.then(() => apply(ev, deps)).finally(() => inFlight.delete(ev.id));
  queue = run.catch(() => undefined);
  return run;
}

async function report(deps: ApplyDeps, id: number, ok: boolean, error?: string): Promise<void> {
  try {
    await deps.confirm(id, ok, error);
  } catch (e) {
    const err = normalizeError(e);
    if (err.code !== "unknown_approval") console.error("confirm_edit failed", err); // unknown = already answered
  }
}

async function apply(ev: ApplyEditEvent, deps: ApplyDeps): Promise<void> {
  const { models: m } = deps;
  const { id, rel } = ev;
  const fail = (msg: string) => report(deps, id, false, msg);
  let touched = false;
  try {
    const before = m.getBufferInfo(rel);
    if (before?.dirty) return fail(`${rel} has unsaved changes in the editor. Save or discard them, then ask again.`);
    if (before?.readOnly) return fail(`${rel} is open read-only in the editor.`);
    if (before?.conflict) return fail(`${rel} changed on disk and is waiting for the user to resolve it.`);
    if (before?.deleted) return fail(`${rel} was deleted from disk.`);

    await m.openFile(rel);
    const info = m.getBufferInfo(rel);
    if (!info || info.status !== "ready") return fail(`${rel} could not be opened in the editor (${info?.status ?? "unknown"}).`);
    if (info.readOnly) return fail(`${rel} is open read-only in the editor.`);
    if (info.dirty) return fail(`${rel} has unsaved changes in the editor. Save or discard them, then ask again.`);

    const current = m.getBuffer(rel) ?? "";
    if (eolFree(current) !== eolFree(ev.old)) {
      return fail(`${rel} changed since the edit was proposed. Read it again and propose the change again.`);
    }
    if (eolFree(current) === eolFree(ev.newText)) {
      await report(deps, id, true); // nothing to change: already identical
      return;
    }

    touched = true;
    m.applyEdit(rel, ev.newText);
    const result = await m.saveFile(rel);
    if (result !== "saved") {
      await m.reloadFromDisk(rel).catch(() => undefined);
      return fail(`${rel} changed on disk before the edit could be saved. Nothing was written.`);
    }
    await report(deps, id, true);
  } catch (e) {
    if (touched) await m.reloadFromDisk(rel).catch(() => undefined);
    await fail(`could not apply the edit to ${rel}: ${normalizeError(e).message}`);
  }
}
