import { useEffect } from "react";
import { Shell } from "./layout/Shell";
import { useStore } from "./state/store";

/** Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 zoom; Ctrl+B / Ctrl+J / Ctrl+Alt+B toggle file / bottom / agent panels. */
export function handleShortcut(e: KeyboardEvent): boolean {
  if (!(e.ctrlKey || e.metaKey)) return false;
  const s = useStore.getState();
  if (e.key === "=" || e.key === "+") s.zoomIn();
  else if (e.key === "-" || e.key === "_") s.zoomOut();
  else if (e.key === "0") s.zoomReset();
  else if (e.key.toLowerCase() === "b" && e.altKey) s.toggleCollapsed("agent");
  else if (e.key.toLowerCase() === "b" && !e.shiftKey) s.toggleCollapsed("filePanel");
  else if (e.key.toLowerCase() === "j") s.toggleCollapsed("bottom");
  else return false;
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
