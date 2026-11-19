import { useEffect } from "react";
import { Shell } from "./layout/Shell";
import { useAgent } from "./agent/agentStore";
import { usePalette } from "./palette/paletteStore";
import { useStore } from "./state/store";

export type ShortcutAction = "zoomIn" | "zoomOut" | "zoomReset" | "toggleFile" | "toggleAgent" | "toggleBottom" | "stopAgent" | "palette";

const EDITABLE = "input, textarea, select, [contenteditable=\"\"], [contenteditable=\"true\"], .xterm, .monaco-editor";

/** Pure: which shortcut (if any) this event is. Exact modifier combos; never AltGr, never in editors/terminals. */
export function shortcutAction(e: KeyboardEvent): ShortcutAction | null {
  if (e.defaultPrevented) return null;
  if (e.getModifierState?.("AltGraph")) return null;
  // Emergency stop works everywhere, including inside the editor and terminals.
  if (e.key === "Escape" && e.ctrlKey && e.altKey && !e.shiftKey && !e.metaKey) return "stopAgent";
  if ((e.key === "p" || e.key === "P") && e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) return "palette";
  const t = e.target;
  if (t instanceof Element && t.closest(EDITABLE)) return null;
  const mod = e.ctrlKey !== e.metaKey; // exactly one of ctrl/cmd
  if (!mod) return null;
  const k = e.key.toLowerCase();
  if (k === "k" && !e.altKey && !e.shiftKey) return "palette";
  if (e.altKey) return k === "b" && !e.shiftKey ? "toggleAgent" : null;
  if (k === "=" || k === "+") return "zoomIn"; // shift is how + is typed on many layouts
  if (k === "-" || k === "_") return "zoomOut";
  if (e.shiftKey) return null;
  if (k === "0") return "zoomReset";
  if (k === "b") return "toggleFile";
  if (k === "j") return "toggleBottom";
  return null;
}

export const shouldHandleShortcut = (e: KeyboardEvent): boolean => shortcutAction(e) !== null;

/** Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 zoom; Ctrl+B file panel, Ctrl+J bottom, Ctrl+Alt+B agent. */
export function handleShortcut(e: KeyboardEvent): boolean {
  const a = shortcutAction(e);
  if (!a) return false;
  if (a === "palette") {
    usePalette.getState().toggle();
    e.preventDefault();
    return true;
  }
  if (a === "stopAgent") {
    void useAgent.getState().cancel();
    e.preventDefault();
    return true;
  }
  const s = useStore.getState();
  if (a === "zoomIn") s.zoomIn();
  else if (a === "zoomOut") s.zoomOut();
  else if (a === "zoomReset") s.zoomReset();
  else if (a === "toggleAgent") s.toggleCollapsed("agent");
  else if (a === "toggleFile") s.toggleCollapsed("filePanel");
  else s.toggleCollapsed("bottom");
  e.preventDefault();
  return true;
}

export default function App() {
  const boot = useStore((s) => s.boot);
  const zoom = useStore((s) => s.zoom);

  useEffect(() => {
    void boot();
  }, [boot]);

  useEffect(() => {
    document.documentElement.style.setProperty("--zoom", String(zoom));
  }, [zoom]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => void handleShortcut(e);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return <Shell />;
}
