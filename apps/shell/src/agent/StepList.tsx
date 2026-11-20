import { Check, Clock, Loader, X, type LucideIcon } from "lucide-react";
import type { Step, StepStatus } from "./agentStore";

const ICON: Record<StepStatus, LucideIcon> = { done: Check, run: Loader, failed: X, todo: Clock };
const LABEL: Record<StepStatus, string> = { done: "done", run: "running", failed: "failed", todo: "pending" };

/** One row per tool call. Long output is already truncated by the reducer. */
export function StepList({ steps }: { steps: Step[] }) {
  if (steps.length === 0) return null;
  return (
    <ol className="steps" aria-label="Agent steps">
      {steps.map((s) => (
        <li key={s.id} className={`step ${s.status}`}>
          <i aria-hidden="true">{(() => { const I = ICON[s.status]; return <I size={14} className={s.status === "run" ? "spin" : undefined} />; })()}</i>
          <div>
            <span className="st-title">{s.title}</span>
            <span className="sr-only"> ({LABEL[s.status]})</span>
            {s.detail && (
              <details className="st-detail">
                <summary>{s.status === "failed" ? "Show error" : "Show output"}</summary>
                <pre className="mono">{s.detail}</pre>
              </details>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}
