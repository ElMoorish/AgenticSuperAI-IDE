import { windowControl } from "../ipc";
import { useModelProbe, useModelStatus } from "../settings/modelStatus";
import { useSettingsDialog } from "../settings/dialogStore";
import { useStore } from "../state/store";
import { THEME_LABELS, THEME_NAMES, isThemeName } from "../styles/themes";

/**
 * Custom title bar: the window is frameless, so the bar is the drag region. Double-click maximize is
 * handled natively by data-tauri-drag-region (no React handler, or it would toggle twice).
 * Drag: brand, folder, search placeholder and model chip. Not drag: theme picker and window buttons.
 */
function win(action: "minimize" | "toggleMaximize" | "close"): void {
  windowControl(action).catch(() => {});
}

export function TitleBar() {
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const folder = useStore((s) => s.workspace.info?.name);
  const model = useStore((s) => s.settings.value?.model);
  const status = useModelStatus((s) => s.state);
  const openSettings = useSettingsDialog((s) => s.open);
  useModelProbe();
  const label = status === "up" ? "connected" : status === "down" ? "server not reachable" : "status unknown";

  return (
    <header className="title" data-tauri-drag-region>
      <span className="brand" data-tauri-drag-region>ASA</span>
      <span className="folder" data-tauri-drag-region>{folder ?? "No folder open"}</span>
      <div className="cmd" data-tauri-drag-region aria-hidden="true">
        <span>Search files, run commands, ask the agent</span>
        <span className="mono">Ctrl+K</span>
      </div>
      <button
        type="button"
        className={`chip chip-${status}`}
        aria-label={`Model ${model || "none"}: ${label}. Open settings`}
        title={status === "down" ? "The model server is not reachable. Click to change it." : "Model and server settings"}
        onClick={openSettings}
      >
        <span className={`dot ${status}`} aria-hidden="true" />
        <span>{model || "no model"}</span>
        {status === "down" && <span className="chip-note">offline</span>}
      </button>
      <label className="themepick">
        Theme
        <select
          value={theme}
          onChange={(e) => {
            if (isThemeName(e.target.value)) void setTheme(e.target.value);
          }}
        >
          {THEME_NAMES.map((n) => (
            <option key={n} value={n}>{THEME_LABELS[n]}</option>
          ))}
        </select>
      </label>
      <div className="winbtns">
        <button type="button" aria-label="Minimize" onClick={() => win("minimize")}>&#8211;</button>
        <button type="button" aria-label="Maximize or restore" onClick={() => win("toggleMaximize")}>&#9633;</button>
        <button type="button" className="close" aria-label="Close" onClick={() => win("close")}>&#10005;</button>
      </div>
    </header>
  );
}
