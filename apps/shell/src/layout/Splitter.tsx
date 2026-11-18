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
  onChange: (px: number) => void;
  onToggle: () => void;
  gridArea?: string;
}

const STEP = 16;
const BIG_STEP = 64;

export function Splitter({ label, orientation, value, min, max, collapsed, sign, onChange, onToggle, gridArea }: Props) {
  const drag = useRef<{ start: number; startValue: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const horizontalBar = orientation === "horizontal";
  const coord = (e: PointerEvent) => (horizontalBar ? e.clientY : e.clientX);
  const current = collapsed ? 0 : value;

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = { start: coord(e), startValue: current };
    setDragging(true);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    onChange(d.startValue + sign * (coord(e) - d.start));
  };
  const end = (e: PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    drag.current = null;
    setDragging(false);
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
      aria-valuenow={current}
      aria-valuemin={min}
      aria-valuemax={max}
      className={`splitter${dragging ? " dragging" : ""}`}
      style={{ gridArea }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onDoubleClick={onToggle}
      onKeyDown={onKeyDown}
    />
  );
}
