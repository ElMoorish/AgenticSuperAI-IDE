/** Where the approval window goes. All numbers are physical pixels (what the window API uses). */

export interface Rect { x: number; y: number; w: number; h: number }
export interface Pos { x: number; y: number }

const MARGIN_LOGICAL = 16;

/** Bottom-right corner of a work area (the screen minus the taskbar), with a margin. */
export function defaultPosition(work: Rect, size: { w: number; h: number }, scale: number): Pos {
  const m = Math.round(MARGIN_LOGICAL * scale);
  return { x: work.x + work.w - size.w - m, y: work.y + work.h - size.h - m };
}

/** True when at least a grabbable part of the window (its top-left 48 px square) is on some screen. */
export function isOnScreen(p: Pos, monitors: Rect[]): boolean {
  const grab = 48;
  return monitors.some((m) => p.x + grab > m.x && p.x < m.x + m.w - grab && p.y >= m.y && p.y < m.y + m.h - grab);
}

/** The remembered position when it is still on a screen, otherwise the default corner. */
export function choosePosition(saved: Pos | null, monitors: Rect[], fallback: Pos): Pos {
  return saved && isOnScreen(saved, monitors) ? saved : fallback;
}

export function parseSaved(raw: string | null): Pos | null {
  try {
    const v = JSON.parse(raw ?? "null");
    return v && Number.isFinite(v.x) && Number.isFinite(v.y) ? { x: Math.round(v.x), y: Math.round(v.y) } : null;
  } catch {
    return null;
  }
}
