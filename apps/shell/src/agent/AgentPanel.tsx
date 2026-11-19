import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { onUiEvent } from "../events";
import { useStore } from "../state/store";
import { ApprovalCard } from "./ApprovalCard";
import { Composer } from "./Composer";
import { Gauges } from "./Gauges";
import { DiffReview } from "./InlineDiff";
import { StepList } from "./StepList";
import { handleApplyEdit } from "./applyEdits";
import { useAgent, type Approval } from "./agentStore";
import "../styles/agent.css";

const STICK_PX = 48;

export function AgentPanel({ collapsed }: { collapsed: boolean }) {
  const { state, contextSize, mode, dispatch, setMode, send, cancel, resolve } = useAgent();
  const hasWorkspace = useStore((s) => !!s.workspace.info);
  const ledgerRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // One host subscription is shared by every panel (events.ts); here we only feed the reducer.
  useEffect(
    () =>
      onUiEvent((ev) => {
        dispatch(ev);
        // An accepted edit of an existing file is applied through the editor buffer (undoable).
        if (ev.type === "applyEdit") void handleApplyEdit(ev);
      }),
    [dispatch],
  );
  const [reviewing, setReviewing] = useState<Approval | null>(null);
  // the card may be answered elsewhere (editor banner, replay): close a review of a gone approval
  const reviewed = reviewing && state.approvals.some((a) => a.id === reviewing.id) ? reviewing : null;

  const busy = state.turn !== "idle";

  // follow the stream unless the user scrolled up to read
  useLayoutEffect(() => {
    const el = ledgerRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [state.messages, state.steps, state.approvals]);

  return (
    <aside className={`agent${collapsed ? " panel-collapsed" : ""}`} aria-label="Agent">
      <div className="ahead">
        <div className="t">
          <span>Agent</span>
          <span className={`turn ${state.turn}`} role="status" aria-live="polite">
            {state.turn === "running" ? "Working…" : state.turn === "waiting" ? "Waiting for you" : ""}
          </span>
        </div>
        <Gauges state={state} contextSize={contextSize} />
      </div>
      {state.banner && (
        <div className="banner warn" role="alert">
          <span>{state.banner}</span>
          <button type="button" onClick={() => dispatch({ type: "dismissBanner" })}>Dismiss</button>
        </div>
      )}
      <div
        className="ledger"
        ref={ledgerRef}
        tabIndex={0}
        aria-label="Conversation"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < STICK_PX;
        }}
      >
        {state.messages.length === 0 && state.steps.length === 0 && (
          <p className="ledger-empty">
            {hasWorkspace ? "Ask the agent to explain, fix or build something in this folder." : "Open a folder to start working with the agent."}
          </p>
        )}
        {state.messages.map((m) => (
          <div key={m.id} className={`msg ${m.role}`}>
            {m.role === "user" && <span className="who">You</span>}
            {m.trimmed && <em className="trimmed">Earlier output is not shown.</em>}
            <div className="text">{m.text}</div>
          </div>
        ))}
        <StepList steps={state.steps} />
        {state.approvals.map((a) => (
          <ApprovalCard key={a.id} approval={a} onResolve={(ok) => void resolve(a.id, ok)} onReview={setReviewing} />
        ))}
      </div>
      <Composer
        mode={mode}
        onMode={setMode}
        busy={busy}
        disabledReason={hasWorkspace ? undefined : "Open a folder first"}
        onSend={(t) => void send(t)}
        onStop={() => void cancel()}
      />
      {reviewed && (
        <DiffReview approval={reviewed} onClose={() => setReviewing(null)} onResolve={(ok) => void resolve(reviewed.id, ok)} />
      )}
    </aside>
  );
}
