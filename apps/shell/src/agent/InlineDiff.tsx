import { useEffect, useRef, useState } from "react";
import { languageFor } from "../editor/models";
import { monacoThemeName } from "../editor/monacoThemes";
import { useStore } from "../state/store";
import type { Approval } from "./agentStore";

/** Modal side-by-side review of an agent edit (Monaco diff editor, lazy-loaded). */
export function DiffReview({
  approval, onClose, onResolve,
}: {
  approval: Approval;
  onClose: () => void;
  onResolve: (accept: boolean) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const theme = useStore((s) => s.theme);
  const fontSize = useStore((s) => s.settings.value?.font_size ?? 13);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLElement>("button")?.focus();
    return () => prev?.focus?.();
  }, []);

  useEffect(() => {
    let disposed = false;
    let viewer: { dispose(): void } | null = null;
    const el = host.current;
    if (!el) return;
    import("../editor/monacoModels")
      .then((m) => {
        if (disposed) return;
        viewer = m.createDiffViewer(el, {
          theme: monacoThemeName(theme),
          fontSize,
          original: approval.old ?? "",
          modified: approval.new ?? "",
          language: languageFor(approval.title),
        });
      })
      .catch((e: unknown) => setError(`The diff viewer could not be loaded: ${e instanceof Error ? e.message : String(e)}`));
    return () => {
      disposed = true;
      viewer?.dispose();
    };
  }, [approval.old, approval.new, approval.title, theme, fontSize]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "Tab") {
      // keep focus inside the dialog: only its buttons (and the diff editor) are tabbable here
      const els = [...(dialog.current?.querySelectorAll<HTMLElement>("button, [tabindex='0'], textarea") ?? [])];
      if (els.length === 0) return;
      const i = els.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (i <= 0 ? els.length - 1 : i - 1) : i === els.length - 1 ? 0 : i + 1;
      e.preventDefault();
      els[next].focus();
    }
  };

  return (
    <div className="modal-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div
        ref={dialog}
        className="modal diff-modal"
        role="dialog"
        aria-modal="true"
        aria-label={`Review edit to ${approval.title}`}
        onKeyDown={onKeyDown}
      >
        <div className="diff-head">
          <b className="mono">{approval.title}</b>
          <span className="diff-legend">Left: current file. Right: the agent&apos;s version.</span>
        </div>
        {error ? <div className="banner err" role="alert"><span>{error}</span></div> : <div className="diff-host" ref={host} />}
        <div className="modal-actions">
          <button type="button" onClick={onClose}>Close</button>
          <button type="button" onClick={() => { onResolve(false); onClose(); }}>Reject edit</button>
          <button type="button" className="primary" onClick={() => { onResolve(true); onClose(); }}>Accept edit</button>
        </div>
      </div>
    </div>
  );
}
