import { useEffect, useRef } from "react";
import type { AgentTerm } from "./agentTerminals";
import { stripAnsi } from "./terminalBuffer";

/** Read-only view of an agent command's output. No input element, no write path. */
export function AgentTerminal({ term }: { term: AgentTerm }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [term.text]);
  return (
    <div className="term-agent">
      <pre ref={ref} className="mono term-out" role="log" aria-label={`Agent terminal ${term.id} output (read-only)`} tabIndex={0}>
        {term.truncated ? "[earlier output was cut]\n" : ""}{stripAnsi(term.text)}
      </pre>
      <div className="term-foot" aria-live="polite">
        {term.exited ? `Exited${term.code === null ? "" : ` with code ${term.code}`}` : "Running (read-only; the agent controls this)"}
      </div>
    </div>
  );
}
