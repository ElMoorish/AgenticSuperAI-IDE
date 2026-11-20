import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import { useAgent } from "../agent/agentStore";
import { isTauri } from "../ipc";
import { EV_APPROVALS, EV_DECIDE, EV_OPEN, EV_READY, parseDecision, toBotItems } from "./relay";

/**
 * IDE-window side of the approval window: sends it the requests that wait, and carries out the answers it
 * sends back through the same `resolve` the cards in the agent panel use (so every path behaves alike).
 */
export function useOverlayRelay(): void {
  useEffect(() => {
    if (!isTauri()) return;
    const offs: Promise<UnlistenFn>[] = [];
    const push = () => void emit(EV_APPROVALS, toBotItems(useAgent.getState().state.approvals)).catch(() => undefined);
    const unsub = useAgent.subscribe((s, prev) => {
      if (s.state.approvals !== prev.state.approvals) push();
    });
    offs.push(listen(EV_READY, push));
    offs.push(
      listen(EV_DECIDE, (e) => {
        const d = parseDecision(e.payload);
        const st = useAgent.getState();
        if (d && st.state.approvals.some((a) => a.id === d.id)) void st.resolve(d.id, d.ok);
      }),
    );
    offs.push(
      listen(EV_OPEN, () => {
        const w = getCurrentWindow();
        void w.unminimize().then(() => w.show()).then(() => w.setFocus()).catch(() => undefined);
      }),
    );
    push();
    return () => {
      unsub();
      offs.forEach((p) => void p.then((f) => f()));
    };
  }, []);
}
