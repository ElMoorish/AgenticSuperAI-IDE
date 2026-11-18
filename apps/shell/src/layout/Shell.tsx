import type { CSSProperties } from "react";
import { LIMITS, useStore, type PanelKey } from "../state/store";
import { Splitter } from "./Splitter";
import { StatusBar } from "./StatusBar";
import { TitleBar } from "./TitleBar";

/**
 * Workbench grid (Tasks 9-13 fill the regions):
 *   .side   = file panel      -> Task 9 (explorer)
 *   .ed     = editor area     -> Task 10/11 (tabs, Monaco)
 *   .bottom = bottom panel    -> Task 12 (terminal, build)
 *   .agent  = agent panel     -> Task 13 (chat, approvals, gauges)
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

  const split = (key: PanelKey, label: string, orientation: "vertical" | "horizontal", sign: 1 | -1) => (
    <Splitter
      label={label}
      orientation={orientation}
      value={sizes[key]}
      min={LIMITS[key].min}
      max={LIMITS[key].max}
      collapsed={collapsed[key]}
      sign={sign}
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
        <h2>FILES</h2>
        <p>Open a folder to browse files.</p>
      </aside>
      {split("filePanel", "Resize file panel", "vertical", 1)}

      <div className="main">
        <section className="ed" aria-label="Editor">
          <div className="tabs"><span>Welcome</span></div>
          <div className="empty">
            <b>No file open</b>
            <span>Pick a file from the explorer, or ask the agent to start a task.</span>
          </div>
        </section>
        {split("bottom", "Resize bottom panel", "horizontal", -1)}
        <section className={`bottom${collapsed.bottom ? " panel-collapsed" : ""}`} aria-label="Bottom panel">
          <div className="ptabs"><b>Terminal</b></div>
          <div className="pbody mono">Terminal and build output will appear here.</div>
        </section>
      </div>

      {split("agent", "Resize agent panel", "vertical", -1)}
      <aside className={`agent${collapsed.agent ? " panel-collapsed" : ""}`} aria-label="Agent">
        <div className="ahead">
          <div className="t"><span>Agent</span></div>
          <div className="gauge">
            <div className="lab"><span>Context</span><b className="mono">- / -</b></div>
            <div className="bar" />
          </div>
          <div className="gauge">
            <div className="lab"><span>GPU memory</span><b className="mono">- / -</b></div>
            <div className="bar" />
          </div>
        </div>
        <div className="ledger">The agent conversation will appear here.</div>
        <div className="composer"><div className="box">Ask the agent or give it a task</div></div>
      </aside>

      <StatusBar />
    </div>
  );
}
