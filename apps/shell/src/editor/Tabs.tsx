import { useEffect, useRef } from "react";
import type { Buffer } from "./models";

const baseName = (rel: string) => rel.slice(rel.lastIndexOf("/") + 1);
const parentName = (rel: string) => {
  const i = rel.lastIndexOf("/");
  if (i < 0) return "";
  const dir = rel.slice(0, i);
  return dir.slice(dir.lastIndexOf("/") + 1);
};

/** Tab label; adds the parent folder when two open tabs share a file name. */
export function tabLabel(rel: string, all: string[]): string {
  const n = baseName(rel);
  const dup = all.some((o) => o !== rel && baseName(o) === n);
  const p = parentName(rel);
  return dup && p ? `${n} \u00b7 ${p}` : n;
}

export function Tabs({ order, buffers, active, onActivate, onClose, onSave }: {
  order: string[];
  buffers: Record<string, Buffer>;
  active: string | null;
  onActivate: (rel: string) => void;
  onClose: (rel: string) => void;
  onSave: (rel: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!active) return;
    const el = [...(listRef.current?.querySelectorAll<HTMLElement>("[role=tab]") ?? [])].find((t) => t.dataset.rel === active);
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active, order.length]);

  const focusTab = (rel: string) => {
    onActivate(rel);
    requestAnimationFrame(() => {
      [...(listRef.current?.querySelectorAll<HTMLElement>("[role=tab]") ?? [])].find((t) => t.dataset.rel === rel)?.focus();
    });
  };

  const onKeyDown = (e: React.KeyboardEvent, rel: string) => {
    const i = order.indexOf(rel);
    const mod = e.ctrlKey || e.metaKey;
    if (mod && !e.altKey && e.key.toLowerCase() === "w") {
      e.preventDefault();
      onClose(rel);
    } else if (mod && !e.altKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      onSave(rel);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      focusTab(order[(i + 1) % order.length]);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      focusTab(order[(i - 1 + order.length) % order.length]);
    } else if (e.key === "Home") {
      e.preventDefault();
      focusTab(order[0]);
    } else if (e.key === "End") {
      e.preventDefault();
      focusTab(order[order.length - 1]);
    } else if (e.key === "Delete") {
      e.preventDefault();
      onClose(rel);
    }
  };

  if (order.length === 0) {
    return <div className="tabs" role="tablist" aria-label="Open files"><span className="tabs-empty">No files open</span></div>;
  }

  return (
    <div className="tabs" role="tablist" aria-label="Open files" ref={listRef}>
      {order.map((rel) => {
        const b = buffers[rel];
        const sel = rel === active;
        const label = tabLabel(rel, order);
        return (
          <div
            key={rel}
            role="tab"
            id={`tab-${rel}`}
            data-rel={rel}
            aria-selected={sel}
            aria-controls="editor-panel"
            tabIndex={sel ? 0 : -1}
            className={`tab${sel ? " sel" : ""}${b?.deleted ? " gone" : ""}`}
            title={rel + (b?.deleted ? " (deleted on disk)" : "")}
            onClick={() => onActivate(rel)}
            onAuxClick={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                onClose(rel);
              }
            }}
            onMouseDown={(e) => { if (e.button === 1) e.preventDefault(); }}
            onKeyDown={(e) => onKeyDown(e, rel)}
          >
            <span className="tname">{label}</span>
            {b?.deleted && <span className="tflag">deleted</span>}
            {b?.dirty && <span className="dot" role="img" aria-label="unsaved changes" />}
            <button
              type="button"
              className="tclose"
              tabIndex={-1}
              aria-label={`Close ${baseName(rel)}`}
              onClick={(e) => { e.stopPropagation(); onClose(rel); }}
            >
              &#215;
            </button>
          </div>
        );
      })}
    </div>
  );
}
