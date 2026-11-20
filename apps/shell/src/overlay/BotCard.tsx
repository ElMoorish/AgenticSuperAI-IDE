import { useEffect, useState } from "react";
import type { BotItem } from "./relay";

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

/**
 * The content of the small approval window: the oldest waiting request with one-click answers.
 * One request at a time, each answered on its own (deliberately no "approve all").
 * The header is the drag handle of the frameless window.
 */
export function BotCard({
  items, onDecide, onOpen,
}: {
  items: BotItem[];
  onDecide: (id: number, ok: boolean) => void;
  onOpen: () => void;
}) {
  const cur = items[0];
  const [answered, setAnswered] = useState<number | null>(null);

  // a fresh request gets fresh buttons
  useEffect(() => {
    if (cur && answered !== null && answered !== cur.id) setAnswered(null);
  }, [cur, answered]);

  if (!cur) return <div className="abot idle" role="status">Nothing is waiting for you.</div>;
  const isEdit = cur.kind === "edit";
  const more = items.length - 1;
  const what = isEdit ? clip(cur.title, 80) : clip(cur.command ?? cur.title, 120);

  const decide = (ok: boolean) => {
    if (answered === cur.id) return; // a double click must not send two decisions
    setAnswered(cur.id);
    onDecide(cur.id, ok);
  };

  return (
    <div className="abot" role="region" aria-label="The agent is waiting for you" aria-live="polite">
      <div className="abot-head" data-tauri-drag-region>
        <span className="abot-face" aria-hidden="true" data-tauri-drag-region>✦</span>
        <span className="abot-title" data-tauri-drag-region>{isEdit ? (cur.isNew ? "New file" : "Edit") : "Run command"}</span>
        {more > 0 && <span className="abot-more" data-tauri-drag-region>+{more} waiting</span>}
        <button type="button" className="abot-open" onClick={onOpen} title="Bring the IDE to the front">Open IDE</button>
      </div>
      <div className="abot-what mono" title={isEdit ? cur.title : cur.command ?? cur.title}>{what}</div>
      {cur.stat && <div className="abot-stat">{cur.stat}</div>}
      <div className="abot-actions">
        <button type="button" className="primary" disabled={answered === cur.id} onClick={() => decide(true)}>
          {isEdit ? "Accept" : "Allow"}
        </button>
        <button type="button" disabled={answered === cur.id} onClick={() => decide(false)}>
          {isEdit ? "Reject" : "Deny"}
        </button>
      </div>
    </div>
  );
}
