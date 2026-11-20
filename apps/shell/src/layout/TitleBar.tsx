import { CircleHelp, Minus, Plug, Search, Square, Unplug, X } from "lucide-react";
import { windowControl } from "../ipc";
import { Brand } from "../ui/Brand";
import { useModelProbe, useModelStatus } from "../settings/modelStatus";
import { useSettingsDialog } from "../settings/dialogStore";
import { usePalette } from "../palette/paletteStore";
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
      <Brand />
      <span className="folder" data-tauri-drag-region>{folder ?? "No folder open"}</span>
      <button type="button" className="cmd" aria-label="Search files and commands (Ctrl+K or Ctrl+P)" onClick={() => usePalette.getState().open()}>
        <Search size={14} aria-hidden="true" />
        <span>Search files and commands</span>
        <span className="mono">Ctrl+K</span>
      </button>
      <button
        type="button"
        className={`chip chip-${status}`}
        aria-label={`Model ${model || "none"}: ${label}. Open settings`}
        title={status === "down" ? "The model server is not reachable. Click to change it." : "Model and server settings"}
        onClick={openSettings}
      >
        {status === "up" ? <Plug size={14} className="sico up" aria-hidden="true" /> : status === "down" ? <Unplug size={14} className="sico down" aria-hidden="true" /> : <CircleHelp size={14} className="sico" aria-hidden="true" />}
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
        <button type="button" aria-label="Minimize" onClick={() => win("minimize")}><Minus size={14} aria-hidden="true" /></button>
        <button type="button" aria-label="Maximize or restore" onClick={() => win("toggleMaximize")}><Square size={12} aria-hidden="true" /></button>
        <button type="button" className="close" aria-label="Close" onClick={() => win("close")}><X size={14} aria-hidden="true" /></button>
      </div>
    </header>
  );
}
