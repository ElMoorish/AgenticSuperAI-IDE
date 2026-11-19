import { useEffect } from "react";
import { create } from "zustand";
import { probeEndpoint } from "../ipc";
import { useStore } from "../state/store";

export type ModelState = "unknown" | "up" | "down";
interface S {
  state: ModelState;
  models: string[];
  error: string | null;
  endpoint: string;
  probe: (url?: string) => Promise<void>;
}

let seq = 0;

export const useModelStatus = create<S>((set) => ({
  state: "unknown",
  models: [],
  error: null,
  endpoint: "",
  async probe(url) {
    const endpoint = url ?? useStore.getState().settings.value?.endpoint ?? "";
    if (!endpoint) { set({ state: "unknown", models: [], error: null, endpoint }); return; }
    const mine = ++seq;
    try {
      const r = await probeEndpoint(endpoint);
      if (mine !== seq) return; // a newer probe superseded this one
      set(r.ok ? { state: "up", models: r.models, error: null, endpoint } : { state: "down", models: [], error: r.error ?? "unreachable", endpoint });
    } catch (e) {
      if (mine !== seq) return;
      const code = (e as { code?: string }).code;
      set({ state: code === "no_tauri" ? "unknown" : "down", models: [], error: e instanceof Error ? e.message : String(e), endpoint });
    }
  },
}));

export const PROBE_INTERVAL_MS = 15_000;

/** Probes the configured endpoint now and every 15 s, but only while the window is visible. */
export function useModelProbe(): void {
  const endpoint = useStore((s) => s.settings.value?.endpoint ?? "");
  useEffect(() => {
    if (!endpoint) return;
    const probe = () => { void useModelStatus.getState().probe(endpoint); };
    let timer: ReturnType<typeof setInterval> | null = null;
    const start = () => { if (timer === null) { probe(); timer = setInterval(probe, PROBE_INTERVAL_MS); } };
    const stop = () => { if (timer !== null) { clearInterval(timer); timer = null; } };
    const onVis = () => (document.visibilityState === "visible" ? start() : stop());
    if (document.visibilityState === "visible") start();
    document.addEventListener("visibilitychange", onVis);
    return () => { stop(); document.removeEventListener("visibilitychange", onVis); };
  }, [endpoint]);
}

export function validateEndpoint(raw: string): string | null {
  const v = raw.trim();
  if (!v) return "Enter the address of your model server, for example http://localhost:11434";
  let u: URL;
  try { u = new URL(v); } catch { return "That is not a valid address. It should look like http://localhost:11434"; }
  if (u.protocol !== "http:" && u.protocol !== "https:") return "The address must start with http:// or https://";
  if (!u.hostname) return "The address needs a host name";
  return null;
}
