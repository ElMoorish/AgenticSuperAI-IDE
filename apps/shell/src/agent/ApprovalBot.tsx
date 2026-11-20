import { useEffect, useMemo, useRef, useState } from "react";
import { DiffReview } from "./InlineDiff";
import { useAgent, type Approval } from "./agentStore";
import { diffLines } from "./diff";
import "../styles/agent.css";

const POS_KEY = "asa.approvalBot.pos";
const MIN_KEY = "asa.approvalBot.min";
const MARGIN = 8;

export interface Pos { x: number; y: number }

/** Keep a dragged position fully inside the window (the window may have been resized since). */
export function clampPos(p: Pos, box: { w: number; h: number }, view: { w: number; h: number }): Pos {
  return {
    x: Math.min(Math.max(MARGIN, p.x), Math.max(MARGIN, view.w - box.w - MARGIN)),
    y: Math.min(Math.max(MARGIN, p.y), Math.max(MARGIN, view.h - box.h - MARGIN)),
  };
}

function loadPos(): Pos | null {
  try {
    const v = JSON.parse(localStorage.getItem(POS_KEY) ?? "null");
    return v && Number.isFinite(v.x) && Number.isFinite(v.y) ? { x: v.x, y: v.y } : null;
  } catch {
    return null;
  }
}

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

/**
 * A small floating helper that shows up while the agent waits for a decision, so Allow/Deny is one click
 * wherever you are in the IDE (even with the agent panel collapsed). One request at a time, oldest first;
 * every request is answered on its own: there is deliberately no "approve all".
 */
export function ApprovalBot() {
  const approvals = useAgent((s) => s.state.approvals);
  const resolve = useAgent((s) => s.resolve);
  const [minimized, setMinimized] = useState(() => {
    try { return localStorage.getItem(MIN_KEY) === "1"; } catch { return false; }
  });
  const [pos, setPos] = useState<Pos | null>(loadPos);
  const [answered, setAnswered] = useState<number | null>(null);
  const [reviewing, setReviewing] = useState<Approval | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ dx: number; dy: number } | null>(null);
  const latest = useRef<Pos | null>(null);

  const cur = approvals[0];
  const isEdit = cur?.kind === "edit";
  const stat = useMemo(() => {
    if (!cur || cur.kind !== "edit") return null;
    const d = diffLines(cur.old ?? "", cur.new ?? "");
    return d.tooLarge ? "large change" : `+${d.added} −${d.removed}`;
  }, [cur]);

  // a new request re-opens a minimized bot's card only via the badge; keep the choice across requests
  useEffect(() => {
    if (cur && answered !== null && answered !== cur.id) setAnswered(null);
  }, [cur, answered]);

  // the window may shrink under a saved position
  useEffect(() => {
    const onResize = () => {
      const el = box.current;
      if (!el || !pos) return;
      const r = el.getBoundingClientRect();
      const next = clampPos(pos, { w: r.width, h: r.height }, { w: window.innerWidth, h: window.innerHeight });
      if (next.x !== pos.x || next.y !== pos.y) setPos(next);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [pos]);

  if (!cur) return null;

  const decide = (ok: boolean) => {
    if (answered === cur.id) return; // a double click must not send two decisions
    setAnswered(cur.id);
    void resolve(cur.id, ok);
  };

  const toggleMin = () => {
    const v = !minimized;
    setMinimized(v);
    try { localStorage.setItem(MIN_KEY, v ? "1" : "0"); } catch { /* ignore */ }
  };

  const startDrag = (e: React.PointerEvent) => {
    const el = box.current;
    if (!el || (e.target as HTMLElement).closest("button")) return;
    const r = el.getBoundingClientRect();
    e.preventDefault(); // no text selection while dragging
    drag.current = { dx: e.clientX - r.left, dy: e.clientY - r.top };
    el.setPointerCapture?.(e.pointerId);
  };
  const moveDrag = (e: React.PointerEvent) => {
    const d = drag.current;
    const el = box.current;
    if (!d || !el) return;
    const r = el.getBoundingClientRect();
    const next = clampPos({ x: e.clientX - d.dx, y: e.clientY - d.dy }, { w: r.width, h: r.height }, { w: window.innerWidth, h: window.innerHeight });
    latest.current = next;
    setPos(next);
  };
  const endDrag = () => {
    if (!drag.current) return;
    drag.current = null;
    try { if (latest.current) localStorage.setItem(POS_KEY, JSON.stringify(latest.current)); } catch { /* ignore */ }
  };

  const style = pos ? { left: pos.x, top: pos.y, right: "auto", bottom: "auto" } : undefined;
  const more = approvals.length - 1;

  return (
    <>
      <div
        ref={box}
        className={`abot${minimized ? " min" : ""}`}
        style={style}
        role="region"
        aria-label="Approval helper"
        aria-live="polite"
      >
        {minimized ? (
          <button type="button" className="abot-bubble" onClick={toggleMin} aria-label={`${approvals.length} request${approvals.length === 1 ? "" : "s"} waiting. Open`} title="The agent is waiting for you">
            <span aria-hidden="true">✦</span>
            <b className="abot-count">{approvals.length}</b>
          </button>
        ) : (
          <>
            <div className="abot-head" onPointerDown={startDrag} onPointerMove={moveDrag} onPointerUp={endDrag} onPointerCancel={endDrag}>
              <span className="abot-face" aria-hidden="true">✦</span>
              <span className="abot-title">{isEdit ? (cur.old ? "Edit" : "New file") : "Run command"}</span>
              {more > 0 && <span className="abot-more">+{more} waiting</span>}
              <button type="button" className="abot-min" onClick={toggleMin} aria-label="Minimize the helper" title="Minimize">–</button>
            </div>
            <div className="abot-what mono" title={isEdit ? cur.title : cur.command ?? cur.title}>
              {isEdit ? clip(cur.title, 80) : clip(cur.command ?? cur.title, 120)}
            </div>
            {stat && <div className="abot-stat">{stat}</div>}
            <div className="abot-actions">
              <button type="button" className="primary" disabled={answered === cur.id} onClick={() => decide(true)}>
                {isEdit ? "Accept" : "Allow"}
              </button>
              <button type="button" disabled={answered === cur.id} onClick={() => decide(false)}>
                {isEdit ? "Reject" : "Deny"}
              </button>
              {isEdit && (
                <button type="button" onClick={() => setReviewing(cur)} title="See the change side by side">Review</button>
              )}
            </div>
          </>
        )}
      </div>
      {reviewing && approvals.some((a) => a.id === reviewing.id) && (
        <DiffReview
          approval={reviewing}
          onClose={() => setReviewing(null)}
          onResolve={(ok) => {
            setReviewing(null);
            void resolve(reviewing.id, ok);
          }}
        />
      )}
    </>
  );
}
