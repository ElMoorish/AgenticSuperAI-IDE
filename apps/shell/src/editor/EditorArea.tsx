import { useCallback, useEffect, useRef, useState } from "react";
import { onUiEvent } from "../events";
import { useStore } from "../state/store";
import { ConfirmDialog } from "./ConfirmDialog";
import { models, type Buffer } from "./models";
import { monacoThemeName } from "./monacoThemes";
import { Tabs } from "./Tabs";
import { useEditorState } from "./useEditorState";
import type { CodeEditor, MonacoModel } from "./monacoModels";

const fmtBytes = (n?: number): string => (n === undefined ? "very large" : n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);
const baseName = (rel: string) => rel.slice(rel.lastIndexOf("/") + 1);
type ViewState = Parameters<CodeEditor["restoreViewState"]>[0];

export function EditorArea() {
  const state = useEditorState();
  const theme = useStore((s) => s.theme);
  const fontSize = useStore((s) => s.settings.value?.font_size);
  const root = useStore((s) => s.workspace.info?.root ?? null);
  const hostRef = useRef<HTMLDivElement>(null);
  const [editor, setEditor] = useState<CodeEditor | null>(null);
  const modRef = useRef<typeof import("./monacoModels") | null>(null);
  const shownRef = useRef<{ rel: string; model: MonacoModel } | null>(null);
  const reloadVs = useRef<Map<string, unknown>>(new Map());
  const [closing, setClosing] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmReload, setConfirmReload] = useState<string | null>(null);

  const active = state.active;
  const buf: Buffer | undefined = active ? state.buffers[active] : undefined;
  const wantsEditor = state.order.some((r) => state.buffers[r]?.status === "ready");

  // workspace switch: drop every buffer of the previous workspace
  useEffect(() => {
    models.reset();
  }, [root]);

  // fs events: reload clean buffers, flag dirty ones. onUiEvent subscribes the host once.
  useEffect(
    () => onUiEvent((ev) => {
      if (ev.type === "fsChanged") void models.handleFsChanged(ev.paths, ev.overflow);
    }),
    [],
  );

  const saveRel = useCallback(async (rel: string | null) => {
    if (!rel) return;
    const b = models.getBufferInfo(rel);
    if (!b || b.status !== "ready" || b.readOnly) return;
    try {
      await models.saveFile(rel);
      setNotice(null);
    } catch (e) {
      setNotice(`Could not save ${baseName(rel)}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, []);

  const requestClose = useCallback((rel: string) => {
    if (models.getBufferInfo(rel)?.dirty) setClosing(rel);
    else models.closeFile(rel);
  }, []);

  const latest = useRef({ saveRel, requestClose });
  latest.current = { saveRel, requestClose };

  // create Monaco lazily, only once a text file is actually open
  useEffect(() => {
    if (!wantsEditor || editor || !hostRef.current) return;
    let cancelled = false;
    const el = hostRef.current;
    void import("./monacoModels").catch((e: unknown) => {
      if (!cancelled) setNotice(`The editor could not be loaded: ${e instanceof Error ? e.message : String(e)}. Reopen the file to retry.`);
      return null;
    }).then((mod) => {
      if (cancelled || !mod) return;
      modRef.current = mod;
      const s = useStore.getState();
      const ed = mod.createEditor(el, { theme: monacoThemeName(s.theme), fontSize: s.settings.value?.font_size ?? 13 });
      const m = mod.monaco;
      // Editor-level shortcuts: the global shortcut guard deliberately ignores .monaco-editor targets.
      ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => void latest.current.saveRel(models.getState().active));
      ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyW, () => {
        const a = models.getState().active;
        if (a) latest.current.requestClose(a);
      });
      setEditor(ed);
    });
    return () => {
      cancelled = true;
    };
  }, [wantsEditor, editor]);

  useEffect(
    () => () => {
      editor?.dispose();
    },
    [editor],
  );

  useEffect(() => {
    if (editor && modRef.current) modRef.current.monaco.editor.setTheme(monacoThemeName(theme));
  }, [editor, theme]);

  useEffect(() => {
    if (editor && fontSize) editor.updateOptions({ fontSize });
  }, [editor, fontSize]);

  // keep cursor/scroll across a silent reload from disk
  useEffect(() => {
    if (!editor) return;
    models.hooks.willReload = (rel) => {
      if (shownRef.current?.rel === rel) reloadVs.current.set(rel, editor.saveViewState());
    };
    models.hooks.willClose = (rel) => {
      // the model is about to be disposed: keep where the user was for a later reopen
      const sh = shownRef.current;
      if (sh && sh.rel === rel && editor.getModel() === sh.model.raw) models.setViewState(rel, editor.saveViewState());
    };
    models.hooks.didReload = (rel) => {
      const vs = reloadVs.current.get(rel);
      reloadVs.current.delete(rel);
      if (vs && shownRef.current?.rel === rel) editor.restoreViewState(vs as ViewState);
    };
    return () => {
      models.hooks = {};
    };
  }, [editor]);

  // show the active buffer's model; save the previous tab's view state; restore this one's
  useEffect(() => {
    if (!editor) return;
    const next = active && buf?.status === "ready" ? (models.getModel(active) as MonacoModel | undefined) : undefined;
    const shown = shownRef.current;
    if (shown && shown.model === next && shown.rel === active) {
      editor.updateOptions({ readOnly: buf?.readOnly ?? false });
      return;
    }
    if (shown && models.getModel(shown.rel) === shown.model && editor.getModel() === shown.model.raw) {
      models.setViewState(shown.rel, editor.saveViewState());
    }
    if (next && active) {
      editor.setModel(next.raw);
      editor.updateOptions({ readOnly: buf?.readOnly ?? false });
      const vs = models.getViewState(active);
      if (vs) editor.restoreViewState(vs as ViewState);
      // Don't steal focus from the agent composer / terminal when a file opens in the background.
      const ae = document.activeElement;
      if (!ae || ae === document.body || hostRef.current?.contains(ae) || (ae as HTMLElement).closest?.(".ed")) editor.focus();
      shownRef.current = { rel: active, model: next };
    } else {
      editor.setModel(null);
      shownRef.current = null;
    }
  }, [editor, active, buf, state]);

  const onWrapperKeyDown = (e: React.KeyboardEvent) => {
    if ((e.target as HTMLElement).closest(".monaco-editor")) return; // handled by Monaco addCommand
    const mod = e.ctrlKey || e.metaKey;
    if (!mod || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "s" && active) {
      e.preventDefault();
      void saveRel(active);
    } else if (k === "w" && active) {
      e.preventDefault();
      requestClose(active);
    }
  };

  const showEditor = !!(buf && buf.status === "ready");

  return (
    <section className="ed" aria-label="Editor" onKeyDown={onWrapperKeyDown}>
      <Tabs
        order={state.order}
        buffers={state.buffers}
        active={active}
        onActivate={(r) => models.activate(r)}
        onClose={requestClose}
        onSave={(r) => void saveRel(r)}
      />
      {buf && buf.status === "ready" && buf.conflict === "disk" && (
        <div className="banner warn" role="alert">
          <span><b>{baseName(buf.rel)}</b> changed on disk. Your unsaved edits were kept.</span>
          <button type="button" onClick={() => setConfirmReload(buf.rel)}>Reload from disk</button>
          <button type="button" onClick={() => void models.overwrite(buf.rel).catch((e: Error) => setNotice(String(e?.message ?? e)))}>Overwrite</button>
          <button type="button" onClick={() => models.keepMine(buf.rel)}>Keep mine</button>
        </div>
      )}
      {buf && buf.status === "ready" && buf.deleted && (
        <div className="banner warn" role="status">
          <span><b>{baseName(buf.rel)}</b> was deleted from disk. Your text is kept in the editor.</span>
          {!buf.readOnly && <button type="button" onClick={() => void saveRel(buf.rel)}>Recreate file</button>}
        </div>
      )}
      {buf && buf.status === "ready" && buf.mixedEol && !buf.readOnly && (
        <div className="banner warn" role="status">
          <span><b>{baseName(buf.rel)}</b> mixes LF and CRLF line endings. Saving after an edit will make them all consistent.</span>
        </div>
      )}
      {notice && (
        <div className="banner err" role="alert">
          <span>{notice}</span>
          <button type="button" onClick={() => setNotice(null)}>Dismiss</button>
        </div>
      )}
      <div id="editor-panel" role="tabpanel" aria-label={active ?? "Editor content"} className="edwrap">
        <div ref={hostRef} className="monaco-host" style={{ display: showEditor ? "block" : "none" }} />
        {!active && (
          <div className="empty">
            <b>No file open</b>
            <span>Pick a file from the explorer, or ask the agent to start a task.</span>
          </div>
        )}
        {buf?.status === "loading" && (
          <div className="empty" role="status" aria-live="polite">
            <b>Opening {baseName(buf.rel)}…</b>
          </div>
        )}
        {buf?.status === "binary" && (
          <div className="empty">
            <b>Binary file, can&apos;t be shown</b>
            <span>{buf.rel}</span>
          </div>
        )}
        {buf?.status === "oversize" && (
          <div className="empty">
            <b>{baseName(buf.rel)} is {fmtBytes(buf.size)}</b>
            {buf.canOpenAnyway ? (
              <>
                <span>Large files can make the editor slow. You can look at it, but not edit it.</span>
                <button type="button" className="primary" onClick={() => void models.openAnyway(buf.rel)}>Open anyway (read-only)</button>
              </>
            ) : (
              <span>This file is over the 10 MB limit and can&apos;t be opened here.</span>
            )}
          </div>
        )}
        {buf?.status === "error" && (
          <div className="empty">
            <b>Couldn&apos;t open {baseName(buf.rel)}</b>
            <span>{buf.error}</span>
            <button type="button" onClick={() => void models.openFile(buf.rel)}>Try again</button>
          </div>
        )}
      </div>
      {confirmReload && state.buffers[confirmReload] && (
        <ConfirmDialog
          title={`Reload ${baseName(confirmReload)} from disk?`}
          message="Your unsaved edits to this file will be discarded."
          actions={[
            { label: "Cancel", value: "cancel", primary: true },
            { label: "Discard my edits and reload", value: "reload" },
          ]}
          onChoose={(v) => {
            const rel = confirmReload;
            setConfirmReload(null);
            if (v === "reload") void models.reloadFromDisk(rel);
          }}
        />
      )}
      {closing && state.buffers[closing] && (
        <ConfirmDialog
          title={`Save changes to ${baseName(closing)}?`}
          message="Your changes will be lost if you don't save them."
          actions={[
            { label: "Save", value: "save", primary: true },
            { label: "Don't save", value: "discard" },
            { label: "Cancel", value: "cancel" },
          ]}
          onChoose={(v) => {
            const rel = closing;
            setClosing(null);
            if (v === "discard") models.closeFile(rel);
            else if (v === "save") {
              void models.saveFile(rel).then((r) => { if (r === "saved" && !models.getBufferInfo(rel)?.dirty) models.closeFile(rel); }).catch((e) => setNotice(`Could not save ${baseName(rel)}: ${e instanceof Error ? e.message : String(e)}`));
            }
          }}
        />
      )}
    </section>
  );
}
