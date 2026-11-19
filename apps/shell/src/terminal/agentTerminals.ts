import { create } from "zustand";
import { onUiEvent } from "../events";
import type { UiEvent } from "../ipc";
import { TerminalBuffer } from "./terminalBuffer";

export interface AgentTerm { id: number; text: string; truncated: boolean; exited: boolean; code: number | null }
interface S { terms: AgentTerm[]; apply(ev: UiEvent): void; remove(id: number): void; clear(): void }

const MAX_TERMS = 12;
const bufs = new Map<number, TerminalBuffer>();

export const useAgentTerminals = create<S>((set) => ({
  terms: [],
  apply(ev) {
    if (ev.type !== "agentTerminalOutput" && ev.type !== "agentTerminalTruncated" && ev.type !== "agentTerminalExit") return;
    set((s) => {
      let terms = s.terms;
      let t = terms.find((x) => x.id === ev.id);
      if (!t) {
        t = { id: ev.id, text: "", truncated: false, exited: false, code: null };
        bufs.set(ev.id, new TerminalBuffer());
        terms = [...terms, t];
        while (terms.length > MAX_TERMS) { bufs.delete(terms[0].id); terms = terms.slice(1); }
      }
      const b = bufs.get(ev.id)!;
      let next = t;
      if (ev.type === "agentTerminalOutput") {
        b.append(ev.chunk);
        next = { ...t, text: b.text(), truncated: t.truncated || b.dropped };
      } else if (ev.type === "agentTerminalTruncated") next = { ...t, truncated: true };
      else next = { ...t, exited: true, code: ev.code };
      return { terms: terms.map((x) => (x.id === ev.id ? next : x)) };
    });
  },
  remove(id) { bufs.delete(id); set((s) => ({ terms: s.terms.filter((t) => t.id !== id) })); },
  clear() { bufs.clear(); set({ terms: [] }); },
}));

let off: (() => void) | null = null;
export function startAgentTerminalFeed(): () => void {
  off?.();
  off = onUiEvent((ev) => useAgentTerminals.getState().apply(ev));
  return () => { off?.(); off = null; };
}
