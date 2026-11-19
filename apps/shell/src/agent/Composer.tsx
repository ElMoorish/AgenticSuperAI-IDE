import { useRef, useState } from "react";
import type { AgentMode } from "../ipc";

const MAX_ROWS = 6;

/**
 * Prompt box: Enter sends, Shift+Enter inserts a newline, Plan/Build switch, Stop while a turn runs.
 * Enter while an IME composition is open (e.g. Japanese, Chinese) confirms the composition instead.
 */
export function Composer({
  mode, onMode, busy, disabledReason, onSend, onStop,
}: {
  mode: AgentMode;
  onMode: (m: AgentMode) => void;
  busy: boolean;
  /** When set, sending is disabled and this text explains why (e.g. no folder open). */
  disabledReason?: string;
  onSend: (text: string) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);
  const blocked = !!disabledReason;

  const grow = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    const line = parseFloat(getComputedStyle(el).lineHeight) || 18;
    el.style.height = `${Math.min(el.scrollHeight, line * MAX_ROWS)}px`;
  };

  const submit = () => {
    const t = text.trim();
    if (!t || busy || blocked) return;
    onSend(t);
    setText("");
    requestAnimationFrame(grow);
  };

  return (
    <div className="composer">
      <div className="modes" role="radiogroup" aria-label="Agent mode">
        {(["plan", "build"] as const).map((m) => (
          <button
            key={m}
            type="button"
            role="radio"
            aria-checked={mode === m}
            className={mode === m ? "on" : undefined}
            onClick={() => onMode(m)}
            title={m === "plan" ? "Plan: the agent reads and proposes, it does not change files" : "Build: the agent may edit files and run commands after you approve"}
          >
            {m === "plan" ? "Plan" : "Build"}
          </button>
        ))}
      </div>
      <div className="box">
        <textarea
          ref={ref}
          rows={1}
          value={text}
          aria-label="Message to the agent"
          placeholder={blocked ? disabledReason : "Ask the agent or give it a task"}
          disabled={blocked}
          onChange={(e) => {
            setText(e.target.value);
            grow();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) {
              e.preventDefault();
              submit();
            }
          }}
        />
        {busy ? (
          <button type="button" className="stop" onClick={onStop} aria-label="Stop the agent">Stop</button>
        ) : (
          <button type="button" className="send primary" onClick={submit} disabled={blocked || text.trim() === ""} aria-label="Send">
            Send
          </button>
        )}
      </div>
      <div className="hint">Enter to send, Shift+Enter for a new line</div>
    </div>
  );
}
