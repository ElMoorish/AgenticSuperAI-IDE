import { Check, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { Approval } from "./agentStore";
import { diffLines, previewRows } from "./diff";

/**
 * A pending agent request that needs the user: an edit (compact diff + side-by-side review) or a
 * command (shown character for character; whitespace and quoting are part of what is approved).
 */
export function ApprovalCard({
  approval, onResolve, onReview,
}: {
  approval: Approval;
  onResolve: (accept: boolean) => void;
  onReview?: (a: Approval) => void;
}) {
  const [answered, setAnswered] = useState(false);
  const isEdit = approval.kind === "edit";
  const diff = useMemo(
    () => (isEdit ? diffLines(approval.old ?? "", approval.new ?? "") : null),
    [isEdit, approval.old, approval.new],
  );
  const preview = useMemo(() => (diff && !diff.tooLarge ? previewRows(diff.ops) : null), [diff]);
  const isNew = isEdit && (approval.old ?? "") === "";

  const answer = (accept: boolean) => {
    if (answered) return; // a double click must not send two decisions
    setAnswered(true);
    onResolve(accept);
  };

  return (
    <div className={`approval ${approval.kind}`} role="group" aria-label={isEdit ? `Approval needed: edit ${approval.title}` : `Approval needed: run a command`}>
      <div className="ap-head">
        <span className="ap-kind">{isEdit ? (isNew ? "New file" : "Edit") : "Run command"}</span>
        {isEdit ? <b className="mono ap-path">{approval.title}</b> : approval.title && <span className="ap-title">{approval.title}</span>}
      </div>

      {approval.kind === "command" && (
        <>
          <pre className="mono ap-command" data-testid="approval-command">{approval.command ?? ""}</pre>
          <div className="ap-note">This runs on your computer, in your project folder.</div>
        </>
      )}

      {diff && (
        <div className="ap-sum" aria-label="Change summary">
          {diff.tooLarge ? (
            <span>
              Large change ({diff.added.toLocaleString("en-US")} lines added, {diff.removed.toLocaleString("en-US")} removed). Too big to preview here;
              use the side-by-side review.
            </span>
          ) : (
            <>
              <span className="add">+{diff.added}</span> <span className="del">−{diff.removed}</span>
              {diff.added === 0 && diff.removed === 0 && <span> (only line endings or whitespace at the end differ)</span>}
            </>
          )}
        </div>
      )}

      {preview && preview.rows.length > 0 && (
        <div className="ap-diff mono" role="region" aria-label="Changed lines" tabIndex={0}>
          {preview.rows.map((r, i) =>
            r.t === "gap" ? (
              <div key={i} className="ln gap">… {r.hidden} unchanged line{r.hidden === 1 ? "" : "s"}</div>
            ) : (
              <div key={i} className={`ln ${r.t}`}>
                <span className="gut" aria-hidden="true">{r.t === "add" ? "+" : r.t === "del" ? "−" : " "}</span>
                <span className="sr-only">{r.t === "add" ? "added: " : r.t === "del" ? "removed: " : ""}</span>
                <span className="tx">{r.text === "" ? " " : r.text}</span>
              </div>
            ),
          )}
          {preview.hiddenChanges > 0 && <div className="ln gap">… {preview.hiddenChanges} more changed lines not shown</div>}
        </div>
      )}

      <div className="actions">
        <button type="button" className="primary" disabled={answered} onClick={() => answer(true)}>
          <Check size={14} aria-hidden="true" />{isEdit ? "Accept edit" : "Allow"}
        </button>
        <button type="button" disabled={answered} onClick={() => answer(false)}>
          <X size={14} aria-hidden="true" />{isEdit ? "Reject edit" : "Deny"}
        </button>
        {isEdit && onReview && (
          <button type="button" onClick={() => onReview(approval)}>Review side by side</button>
        )}
      </div>
    </div>
  );
}
