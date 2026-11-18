/**
 * Single fan-out for host UiEvents. `subscribe_events` is invoked once (lazily, on the first
 * listener), so React strict-mode double mounts and several consumers (file tree, editor,
 * agent panel) never create duplicate host subscriptions. Outside Tauri it is a no-op.
 */
import { subscribeEvents, type UiEvent } from "./ipc";

type Handler = (ev: UiEvent) => void;
const handlers = new Set<Handler>();
let started = false;

export function dispatchUiEvent(ev: UiEvent): void {
  for (const h of [...handlers]) {
    try {
      h(ev);
    } catch (e) {
      console.error("ui event handler failed", e);
    }
  }
}

/** Returns an unsubscribe function. */
export function onUiEvent(h: Handler): () => void {
  handlers.add(h);
  if (!started) {
    started = true;
    subscribeEvents(dispatchUiEvent).catch((e: { code?: string }) => {
      started = false; // retry on the next listener (not in Tauri, or the host was not ready)
      if (e?.code !== "no_tauri") console.error("subscribe_events failed", e);
    });
  }
  return () => void handlers.delete(h);
}

/** Test helper. */
export function resetUiEventsForTest(): void {
  handlers.clear();
  started = false;
}
