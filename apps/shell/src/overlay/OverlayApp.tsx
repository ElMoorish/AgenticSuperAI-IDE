import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { availableMonitors, getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect, useRef, useState } from "react";
import { BotCard } from "./BotCard";
import { choosePosition, defaultPosition, parseSaved, type Rect } from "./placement";
import { EV_APPROVALS, EV_DECIDE, EV_OPEN, EV_READY, type BotItem } from "./relay";

const POS_KEY = "asa.approvalOverlay.pos";
/** Moves we make ourselves (placing the window) are not "the user moved it". */
const SETTLE_MS = 700;
const POLL_MS = 400;

const rectOf = (m: { position: { x: number; y: number }; size: { width: number; height: number } }): Rect => ({
  x: m.position.x, y: m.position.y, w: m.size.width, h: m.size.height,
});

/**
 * The small always-on-top window. It is hidden unless the agent waits for an answer AND the IDE window is
 * not the one being used (another app in front, or the IDE minimized). It appears at the bottom-right of the
 * screen, or where the user last dragged it. It never takes the keyboard focus from the app being used.
 */
export default function OverlayApp() {
  const [items, setItems] = useState<BotItem[]>([]);
  const [mainFocused, setMainFocused] = useState(true);
  const ignoreMovesUntil = useRef(0);
  const saveTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const me = getCurrentWindow();
    const offs: Promise<UnlistenFn>[] = [];
    void me.setFocusable(false).catch(() => undefined);

    offs.push(listen<BotItem[]>(EV_APPROVALS, (e) => setItems(Array.isArray(e.payload) ? e.payload : [])));
    void emit(EV_READY);

    void (async () => {
      const main = await WebviewWindow.getByLabel("main");
      if (!main) return;
      setMainFocused(await main.isFocused().catch(() => true));
      offs.push(main.onFocusChanged((e) => setMainFocused(e.payload)));
    })();

    // remember where the user drags it
    offs.push(
      me.onMoved((e) => {
        if (Date.now() < ignoreMovesUntil.current) return;
        const p = { x: e.payload.x, y: e.payload.y };
        window.clearTimeout(saveTimer.current);
        saveTimer.current = window.setTimeout(() => {
          try { localStorage.setItem(POS_KEY, JSON.stringify(p)); } catch { /* ignore */ }
        }, 400);
      }),
    );
    return () => {
      window.clearTimeout(saveTimer.current);
      offs.forEach((p) => void p.then((f) => f()));
    };
  }, []);

  // Focus events from another window are not always delivered, so while something waits also look every
  // so often: the window is "in use" only when the IDE has the focus (a minimized one does not).
  const waiting = items.length > 0;
  useEffect(() => {
    if (!waiting) return;
    let alive = true;
    const look = async () => {
      const main = await WebviewWindow.getByLabel("main");
      if (!main) return;
      const [focused, minimized] = await Promise.all([main.isFocused().catch(() => true), main.isMinimized().catch(() => false)]);
      if (alive) setMainFocused(focused && !minimized);
    };
    void look();
    const t = window.setInterval(() => void look(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [waiting]);

  const visible = waiting && !mainFocused;
  useEffect(() => {
    const me = getCurrentWindow();
    if (!visible) {
      void me.hide();
      return;
    }
    void (async () => {
      try {
        const [prim, all, size] = await Promise.all([primaryMonitor(), availableMonitors(), me.outerSize()]);
        const scale = prim?.scaleFactor ?? 1;
        const work = prim ? rectOf(prim.workArea) : { x: 0, y: 0, w: 1920, h: 1080 };
        const fallback = defaultPosition(work, { w: size.width, h: size.height }, scale);
        let saved = null;
        try { saved = parseSaved(localStorage.getItem(POS_KEY)); } catch { /* ignore */ }
        const pos = choosePosition(saved, all.map((m) => rectOf(m.workArea)), fallback);
        ignoreMovesUntil.current = Date.now() + SETTLE_MS;
        await me.setPosition(new PhysicalPosition(pos.x, pos.y));
      } catch {
        /* placement is best effort: still show the window */
      }
      await me.show();
    })();
  }, [visible]);

  return (
    <BotCard
      items={items}
      onDecide={(id, ok) => void emit(EV_DECIDE, { id, ok })}
      onOpen={() => void emit(EV_OPEN)}
    />
  );
}
