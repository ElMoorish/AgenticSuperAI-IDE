import { useEffect, useRef } from "react";

export interface ConfirmAction { label: string; value: string; primary?: boolean }

/** In-app modal (never window.confirm). Esc cancels (onChoose(null)); Tab stays inside. */
export function ConfirmDialog({ title, message, actions, onChoose }: {
  title: string;
  message: string;
  actions: ConfirmAction[];
  onChoose: (value: string | null) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>("button.primary") ?? ref.current?.querySelector<HTMLElement>("button");
    first?.focus();
    return () => prev?.focus?.();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onChoose(null);
    } else if (e.key === "Tab") {
      const btns = [...(ref.current?.querySelectorAll<HTMLElement>("button") ?? [])];
      if (btns.length === 0) return;
      const i = btns.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (i <= 0 ? btns.length - 1 : i - 1) : i === btns.length - 1 ? 0 : i + 1;
      e.preventDefault();
      btns[next].focus();
    }
  };

  return (
    <div className="modal-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onChoose(null); }}>
      <div ref={ref} className="modal" role="alertdialog" aria-modal="true" aria-labelledby="dlg-title" aria-describedby="dlg-msg" onKeyDown={onKeyDown}>
        <h3 id="dlg-title">{title}</h3>
        <p id="dlg-msg">{message}</p>
        <div className="modal-actions">
          {actions.map((a) => (
            <button key={a.value} type="button" className={a.primary ? "primary" : undefined} onClick={() => onChoose(a.value)}>{a.label}</button>
          ))}
        </div>
      </div>
    </div>
  );
}
