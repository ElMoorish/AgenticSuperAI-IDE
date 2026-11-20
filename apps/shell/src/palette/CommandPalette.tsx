import { useEffect, useMemo, useRef, useState } from "react";
import { useAgent } from "../agent/agentStore";
import { models } from "../editor/models";
import { searchFiles } from "../ipc";
import { useSettingsDialog } from "../settings/dialogStore";
import { useModelsDialog } from "../models/ModelsDialog";
import { useStore } from "../state/store";
import { usePalette } from "./paletteStore";

interface Item { key: string; label: string; hint?: string; run: () => void }

/**
 * Quick open and commands. Type to find a file by name; start with ">" for commands.
 * Up/Down choose, Enter runs, Escape closes. Search runs in the host (bounded, ignores node_modules etc).
 */
export function CommandPalette() {
  const close = usePalette((s) => s.close);
  const hasFolder = useStore((s) => s.workspace.info !== null);
  const [q, setQ] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [sel, setSel] = useState(0);
  const [msg, setMsg] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    input.current?.focus();
    return () => prev?.focus?.();
  }, []);

  const commandMode = q.startsWith(">");
  useEffect(() => {
    if (commandMode || !hasFolder) { setFiles([]); setMsg(null); return; }
    const mine = ++seq.current;
    const t = setTimeout(() => {
      searchFiles(q.trim(), 50)
        .then((r) => { if (mine === seq.current) { setFiles(r); setMsg(null); } })
        .catch((e: unknown) => { if (mine === seq.current) { setFiles([]); setMsg(e instanceof Error ? e.message : String(e)); } });
    }, 80);
    return () => clearTimeout(t);
  }, [q, commandMode, hasFolder]);

  const commands: Item[] = useMemo(() => {
    const st = () => useStore.getState();
    const list: Array<[string, string, () => void]> = [
      ["Open settings", "", () => useSettingsDialog.getState().open()],
      ["Find and start a model", "", () => useModelsDialog.getState().open()],
      ["Toggle file explorer", "Ctrl+B", () => st().toggleCollapsed("filePanel")],
      ["Toggle terminal panel", "Ctrl+J", () => st().toggleCollapsed("bottom")],
      ["Toggle agent panel", "Ctrl+Alt+B", () => st().toggleCollapsed("agent")],
      ["Stop the agent", "Ctrl+Alt+Esc", () => void useAgent.getState().cancel()],
      ["Zoom in", "Ctrl+=", () => st().zoomIn()],
      ["Zoom out", "Ctrl+-", () => st().zoomOut()],
      ["Reset zoom", "Ctrl+0", () => st().zoomReset()],
    ];
    return list.map(([label, hint, run]) => ({ key: label, label, hint, run }));
  }, []);

  const items: Item[] = useMemo(() => {
    if (commandMode) {
      const term = q.slice(1).trim().toLowerCase();
      return commands.filter((c) => c.label.toLowerCase().includes(term));
    }
    return files.map((rel) => ({
      key: rel,
      label: rel.split("/").pop() ?? rel,
      hint: rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "",
      run: () => void models.openFile(rel),
    }));
  }, [commandMode, q, commands, files]);

  useEffect(() => setSel(0), [items.length, q]);

  const choose = (it: Item | undefined) => {
    if (!it) return;
    close();
    it.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(items.length - 1, s + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(0, s - 1)); }
    else if (e.key === "Enter") { e.preventDefault(); choose(items[sel]); }
  };

  const empty = commandMode
    ? "No matching command"
    : !hasFolder
      ? "Open a folder first (then type a file name). Type > for commands."
      : msg ?? (q.trim() ? "No matching files" : "Type a file name. Type > for commands.");

  return (
    <div className="modal-scrim palette-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Search files and commands" onKeyDown={onKeyDown}>
        <input
          ref={input}
          className="palette-input"
          type="text"
          role="combobox"
          aria-expanded={items.length > 0}
          aria-controls="palette-list"
          aria-activedescendant={items[sel] ? `pal-${sel}` : undefined}
          placeholder="Search files, or type > for commands"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          spellCheck={false}
        />
        {items.length === 0 ? (
          <div className="palette-empty" role="status">{empty}</div>
        ) : (
          <ul id="palette-list" className="palette-list" role="listbox">
            {items.map((it, i) => (
              <li
                key={it.key}
                id={`pal-${i}`}
                role="option"
                aria-selected={i === sel}
                className={i === sel ? "sel" : ""}
                onMouseMove={() => setSel(i)}
                onMouseDown={(e) => { e.preventDefault(); choose(it); }}
              >
                <span className="pl">{it.label}</span>
                {it.hint && <span className="ph mono">{it.hint}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
