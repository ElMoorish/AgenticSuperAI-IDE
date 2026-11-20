import { Send, Square } from "lucide-react";
import { useRef, useState } from "react";
import type { AgentMode } from "../ipc";
import { IMAGE_TYPES, MAX_IMAGES, imageFiles, prepareImage, type Attachment } from "./images";

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
  onSend: (text: string, images?: Attachment[]) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const [images, setImages] = useState<Attachment[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const blocked = !!disabledReason;

  const grow = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    const line = parseFloat(getComputedStyle(el).lineHeight) || 18;
    el.style.height = `${Math.min(el.scrollHeight, line * MAX_ROWS)}px`;
  };

  const attach = async (files: File[]) => {
    if (files.length === 0 || blocked) return;
    setProblem(null);
    const room = MAX_IMAGES - images.length;
    if (room <= 0) {
      setProblem(`At most ${MAX_IMAGES} pictures per message.`);
      return;
    }
    if (files.length > room) setProblem(`At most ${MAX_IMAGES} pictures per message; the rest were skipped.`);
    const done: Attachment[] = [];
    for (const f of files.slice(0, room)) {
      try {
        done.push(await prepareImage(f, f.name || "pasted image"));
      } catch (e) {
        setProblem(e instanceof Error ? e.message : "Could not read that picture.");
      }
    }
    if (done.length) setImages((cur) => [...cur, ...done].slice(0, MAX_IMAGES));
  };

  const submit = () => {
    const t = text.trim();
    if ((!t && images.length === 0) || busy || blocked) return;
    if (images.length) onSend(t, images);
    else onSend(t);
    setText("");
    setImages([]);
    setProblem(null);
    requestAnimationFrame(grow);
  };

  return (
    <div
      className={`composer${over ? " drop" : ""}`}
      onDragOver={(e) => {
        if (!blocked && Array.from(e.dataTransfer?.types ?? []).includes("Files")) {
          e.preventDefault();
          setOver(true);
        }
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        setOver(false);
        const files = imageFiles(e.dataTransfer?.files);
        if (files.length) {
          e.preventDefault();
          void attach(files);
        }
      }}
    >
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
      {images.length > 0 && (
        <ul className="attached" aria-label="Attached pictures">
          {images.map((im) => (
            <li key={im.id}>
              <img src={im.url} alt={im.name} />
              <button type="button" aria-label={`Remove ${im.name}`} onClick={() => setImages((cur) => cur.filter((x) => x.id !== im.id))}>
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      {problem && <div className="attach-problem" role="alert">{problem}</div>}
      <div className="box">
        <input
          ref={picker}
          type="file"
          accept={IMAGE_TYPES.join(",")}
          multiple
          hidden
          aria-label="Choose pictures"
          onChange={(e) => {
            void attach(imageFiles(e.target.files));
            e.target.value = "";
          }}
        />
        <button
          type="button"
          className="attach"
          disabled={blocked || busy}
          aria-label="Attach a picture"
          title="Attach a picture (or paste or drop one). The model must support images."
          onClick={() => picker.current?.click()}
        >
          +
        </button>
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
          onPaste={(e) => {
            const files = imageFiles(e.clipboardData?.files);
            if (files.length) {
              e.preventDefault(); // a screenshot has no text worth pasting
              void attach(files);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) {
              e.preventDefault();
              submit();
            }
          }}
        />
        {busy ? (
          <button type="button" className="stop" onClick={onStop} aria-label="Stop the agent"><Square size={13} aria-hidden="true" />Stop</button>
        ) : (
          <button type="button" className="send primary" onClick={submit} disabled={blocked || (text.trim() === "" && images.length === 0)} aria-label="Send">
            <Send size={14} aria-hidden="true" />Send
          </button>
        )}
      </div>
      <div className="hint">Enter to send, Shift+Enter for a new line</div>
    </div>
  );
}
