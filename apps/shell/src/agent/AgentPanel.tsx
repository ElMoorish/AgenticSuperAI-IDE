import { useEffect, useLayoutEffect, useRef } from "react";
import { onUiEvent } from "../events";
import { useStore } from "../state/store";
import { Composer } from "./Composer";
import { Gauges } from "./Gauges";
import { StepList } from "./StepList";
import { useAgent } from "./agentStore";
import "../styles/agent.css";

const STICK_PX = 48;

export function AgentPanel({ collapsed }: { collapsed: boolean }) {
  const { state, contextSize, mode, dispatch, setMode, send, cancel, resolve } = useAgent();
  const hasWorkspace = useStore((s) => !!s.workspace.info);
  const ledgerRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // One host subscription is shared by every panel (events.ts); here we only feed the reducer.
  useEffect(() => onUiEvent((ev) => dispatch(ev)), [dispatch]);

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
          // Task 11 replaces this with the full ApprovalCard (diff, exact command line).
          <div key={a.id} className="approval" role="group" aria-label={`Approval needed: ${a.title}`}>
            <b>{a.title}</b>
            {a.command && <pre className="mono">{a.command}</pre>}
            <div className="actions">
              <button type="button" className="primary" onClick={() => void resolve(a.id, true)}>Accept</button>
              <button type="button" onClick={() => void resolve(a.id, false)}>Reject</button>
            </div>
          </div>
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
    </aside>
  );
}
