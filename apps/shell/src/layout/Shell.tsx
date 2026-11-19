import type { CSSProperties } from "react";
import { AgentPanel } from "../agent/AgentPanel";
import { EditorArea } from "../editor/EditorArea";
import { FileTree } from "../files/FileTree";
import { LIMITS, useStore, type PanelKey } from "../state/store";
import { Splitter } from "./Splitter";
import { StatusBar } from "./StatusBar";
import { TerminalPanel } from "../terminal/TerminalPanel";
import { TitleBar } from "./TitleBar";

/**
 * Workbench grid (Tasks 9-13 fill the regions):
 *   .side   = file panel      -> Task 9 (file tree)
 *   .ed     = editor area     -> Task 9 (tabs, Monaco)
 *   .bottom = bottom panel    -> Task 12 (terminal, build)
 *   .agent  = agent panel     -> Task 10 (chat, steps, gauges, composer); Task 11 approval cards
 */
export function Shell() {
  const { sizes, collapsed } = useStore((s) => s.layout);
  const setSize = useStore((s) => s.setSize);
  const toggle = useStore((s) => s.toggleCollapsed);

  const px = (k: PanelKey) => (collapsed[k] ? 0 : sizes[k]);
  const style = {
    "--file-w": `${px("filePanel")}px`,
    "--agent-w": `${px("agent")}px`,
    "--bottom-h": `${px("bottom")}px`,
  } as CSSProperties;

  const VARS: Record<PanelKey, string> = { filePanel: "--file-w", agent: "--agent-w", bottom: "--bottom-h" };
  const split = (key: PanelKey, label: string, orientation: "vertical" | "horizontal", sign: 1 | -1) => (
    <Splitter
      label={label}
      orientation={orientation}
      value={sizes[key]}
      min={LIMITS[key].min}
      max={LIMITS[key].max}
      collapsed={collapsed[key]}
      sign={sign}
      cssVar={VARS[key]}
      onChange={(v) => setSize(key, v)}
      onToggle={() => toggle(key)}
    />
  );

  return (
    <div className="ide" style={style}>
      <TitleBar />

      <nav className="rail" aria-label="Activity bar">
        <button type="button" aria-label="Explorer" aria-pressed={!collapsed.filePanel} title="Explorer (Ctrl+B)" onClick={() => toggle("filePanel")}>&#9636;</button>
        <button type="button" aria-label="Search" title="Search (coming later)" disabled>&#8981;</button>
        <button type="button" aria-label="Settings" title="Settings (coming later)" disabled>&#9881;</button>
        <button type="button" aria-label="Run" title="Run (coming later)" disabled>&#9662;</button>
      </nav>

      <aside className={`side${collapsed.filePanel ? " panel-collapsed" : ""}`} aria-label="Files">
        <FileTree />
      </aside>
      {split("filePanel", "Resize file panel", "vertical", 1)}

      <div className="main">
        <EditorArea />
        {split("bottom", "Resize bottom panel", "horizontal", -1)}
        <section className={`bottom${collapsed.bottom ? " panel-collapsed" : ""}`} aria-label="Bottom panel">
          <TerminalPanel />
        </section>
      </div>

      {split("agent", "Resize agent panel", "vertical", -1)}
      <AgentPanel collapsed={collapsed.agent} />

      <StatusBar />
    </div>
  );
}
