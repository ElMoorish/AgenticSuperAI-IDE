import { useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

interface Props {
  label: string;
  /** "vertical" separator = a vertical bar that resizes widths (left/right). */
  orientation: "vertical" | "horizontal";
  value: number;
  min: number;
  max: number;
  collapsed: boolean;
  /** +1 if moving toward +x/+y grows the panel, -1 if it shrinks it. */
  sign: 1 | -1;
  /** CSS variable on `.ide` that carries this panel's size (updated live while dragging). */
  cssVar: string;
  /** Commit a final size (pointerup or keyboard step). Not called per pointermove. */
  onChange: (px: number) => void;
  onToggle: () => void;
}

const STEP = 16;
const BIG_STEP = 64;

export function Splitter({ label, orientation, value, min, max, collapsed, sign, cssVar, onChange, onToggle }: Props) {
  const drag = useRef<{ start: number; startValue: number; latest: number; target: HTMLElement | null } | null>(null);
  const [dragging, setDragging] = useState(false);
  const horizontalBar = orientation === "horizontal";
  const coord = (e: PointerEvent) => (horizontalBar ? e.clientY : e.clientX);
  const current = collapsed ? 0 : value;
  const clamp = (v: number) => Math.min(max, Math.max(min, Math.round(v)));

  const finish = (commit: boolean) => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    setDragging(false);
    document.body.classList.remove("resizing");
    if (commit && d.latest !== d.startValue) onChange(d.latest);
    else d.target?.style.setProperty(cssVar, `${current}px`); // cancelled / unmoved: restore
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = {
      start: coord(e),
      startValue: current,
      latest: current,
      target: e.currentTarget.closest<HTMLElement>(".ide"),
    };
    setDragging(true);
    document.body.classList.add("resizing");
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    if (e.buttons === 0) return finish(true); // button released outside the window
    d.latest = clamp(d.startValue + sign * (coord(e) - d.start));
    d.target?.style.setProperty(cssVar, `${d.latest}px`); // live, no React re-render
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    finish(true); // commit before releasing capture (release fires lostpointercapture)
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const plus = horizontalBar ? "ArrowDown" : "ArrowRight";
    const minus = horizontalBar ? "ArrowUp" : "ArrowLeft";
    const step = e.shiftKey ? BIG_STEP : STEP;
    if (e.key === plus) onChange(current + sign * step);
    else if (e.key === minus) onChange(current - sign * step);
    else if (e.key === "Home") onChange(min);
    else if (e.key === "End") onChange(max);
    else if (e.key === "Enter" || e.key === " ") onToggle();
    else return;
    e.preventDefault();
  };

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-orientation={orientation}
      aria-valuenow={value}
      aria-valuetext={collapsed ? "collapsed" : undefined}
      aria-valuemin={min}
      aria-valuemax={max}
      className={`splitter${dragging ? " dragging" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => finish(false)}
      onLostPointerCapture={() => finish(false)}
      onDoubleClick={onToggle}
      onKeyDown={onKeyDown}
    />
  );
}
