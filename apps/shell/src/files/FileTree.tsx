import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ConfirmDialog } from "../editor/ConfirmDialog";
import { models } from "../editor/models";
import { useEditorState } from "../editor/useEditorState";
import { onUiEvent } from "../events";
import { listDir, openFolder, pickFolder, type DirEntry } from "../ipc";
import { useStore } from "../state/store";

export const ROW_REM = 1.5; // row height in rem (zoom scales the root font size)
const OVERSCAN = 8;
const FS_DEBOUNCE_MS = 150;
const FS_MANY_PATHS = 100;
const FALLBACK_VIEW_H = 480;

export interface DirState { status: "loading" | "ok" | "error"; entries: DirEntry[]; error?: string }
export type Row =
  | { kind: "entry"; e: DirEntry; level: number; pos: number; size: number }
  | { kind: "msg"; key: string; level: number; text: string };

export const parentOf = (rel: string): string => (rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "");
const norm = (p: string) => p.replace(/\\/g, "/").replace(/^\.?\//, "").replace(/\/+$/, "");

/** Flatten the loaded/expanded directories into the visible row list (pure). */
export function flatten(dirs: Record<string, DirState>, expanded: ReadonlySet<string>): Row[] {
  const rows: Row[] = [];
  const walk = (rel: string, level: number) => {
    const d = dirs[rel];
    if (!d) return;
    d.entries.forEach((e, i) => {
      rows.push({ kind: "entry", e, level, pos: i + 1, size: d.entries.length });
      if (e.isDir && expanded.has(e.rel)) {
        walk(e.rel, level + 1);
        const c = dirs[e.rel];
        if (!c || (c.status === "loading" && c.entries.length === 0)) rows.push({ kind: "msg", key: e.rel + "\0l", level: level + 1, text: "Loading…" });
        else if (c.status === "error") rows.push({ kind: "msg", key: e.rel + "\0e", level: level + 1, text: `Couldn't read this folder: ${c.error ?? "error"}` });
        else if (c.entries.length === 0) rows.push({ kind: "msg", key: e.rel + "\0m", level: level + 1, text: "Empty folder" });
      }
    });
  };
  walk("", 1);
  return rows;
}

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
const errCode = (e: unknown) => (e && typeof e === "object" ? (e as { code?: string }).code : undefined);

export function FileTree() {
  const info = useStore((s) => s.workspace.info);
  const recent = useStore((s) => s.settings.value?.recent);
  const setWorkspace = useStore((s) => s.setWorkspace);
  const zoom = useStore((s) => s.zoom);
  const editor = useEditorState();
  const root = info?.root ?? null;
  const rowH = ROW_REM * 16 * zoom;

  const [dirs, setDirs] = useState<Record<string, DirState>>({});
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [focusRel, setFocusRel] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(FALLBACK_VIEW_H);
  const [openErr, setOpenErr] = useState<string | null>(null);
  const [confirmPath, setConfirmPath] = useState<string | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const gen = useRef(0);
  const dirsRef = useRef(dirs);
  dirsRef.current = dirs;
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const pendingFocus = useRef(false);

  const load = useCallback(async (rel: string, g: number) => {
    try {
      const entries = await listDir(rel);
      if (g !== gen.current) return;
      setDirs((d) => ({ ...d, [rel]: { status: "ok", entries } }));
    } catch (e) {
      if (g !== gen.current) return;
      if (errCode(e) === "not_found" && rel !== "") {
        const gone = (r: string) => r === rel || r.startsWith(rel + "/");
        setDirs((d) => Object.fromEntries(Object.entries(d).filter(([k]) => !gone(k))));
        setExpanded((x) => new Set([...x].filter((r) => !gone(r))));
      } else {
        setDirs((d) => ({ ...d, [rel]: { status: "error", entries: d[rel]?.entries ?? [], error: errMsg(e) } }));
      }
    }
  }, []);

  // workspace changed: start over (stale loads are dropped by the generation guard)
  useEffect(() => {
    const g = ++gen.current;
    setExpanded(new Set());
    setFocusRel(null);
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    if (root === null) {
      setDirs({});
      return;
    }
    setDirs({ "": { status: "loading", entries: [] } });
    void load("", g);
  }, [root, load]);

  // viewport height
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => setViewH(el.clientHeight || FALLBACK_VIEW_H);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [root]);

  // FsChanged: debounce, refresh only the affected (loaded + visible) directories
  const pending = useRef<{ paths: Set<string>; all: boolean; timer: ReturnType<typeof setTimeout> | null }>({ paths: new Set(), all: false, timer: null });
  useEffect(() => {
    const p = pending.current;
    const flush = () => {
      p.timer = null;
      const visible = Object.keys(dirsRef.current).filter((k) => k === "" || expandedRef.current.has(k));
      let targets: string[];
      if (p.all || p.paths.size > FS_MANY_PATHS) targets = visible;
      else {
        const parents = new Set([...p.paths].map(parentOf));
        targets = visible.filter((k) => parents.has(k));
      }
      p.paths = new Set();
      p.all = false;
      const g = gen.current;
      for (const rel of targets) void load(rel, g);
    };
    const off = onUiEvent((ev) => {
      if (ev.type !== "fsChanged") return;
      if (ev.overflow) p.all = true;
      for (const x of ev.paths) p.paths.add(norm(x));
      if (!p.timer) p.timer = setTimeout(flush, FS_DEBOUNCE_MS);
    });
    return () => {
      off();
      if (p.timer) clearTimeout(p.timer);
      p.timer = null;
    };
  }, [load]);

  const rows = useMemo(() => flatten(dirs, expanded), [dirs, expanded]);
  const navRows = useMemo(() => rows.filter((r): r is Extract<Row, { kind: "entry" }> => r.kind === "entry"), [rows]);
  const rowIndex = useMemo(() => {
    const m = new Map<string, number>();
    rows.forEach((r, i) => { if (r.kind === "entry") m.set(r.e.rel, i); });
    return m;
  }, [rows]);

  const tabStop = focusRel !== null && rowIndex.has(focusRel) ? focusRel : (navRows[0]?.e.rel ?? null);

  const toggle = useCallback((rel: string) => {
    const open = expandedRef.current.has(rel);
    setExpanded((x) => {
      const n = new Set(x);
      if (open) n.delete(rel);
      else n.add(rel);
      return n;
    });
    if (!open) {
      setDirs((d) => (d[rel] ? d : { ...d, [rel]: { status: "loading", entries: [] } }));
      void load(rel, gen.current); // always revalidate on expand
    }
  }, [load]);

  const openEntry = useCallback((e: DirEntry) => {
    void models.openFile(e.rel, { size: e.size ?? undefined });
  }, []);

  const ensureVisible = (idx: number) => {
    const el = scrollRef.current;
    if (!el) return;
    let top = el.scrollTop;
    if (idx * rowH < top) top = idx * rowH;
    else if ((idx + 1) * rowH > top + viewH) top = (idx + 1) * rowH - viewH;
    if (top !== el.scrollTop) {
      el.scrollTop = top;
      setScrollTop(top);
    }
  };

  const moveFocus = (rel: string) => {
    const idx = rowIndex.get(rel);
    if (idx === undefined) return;
    pendingFocus.current = true;
    setFocusRel(rel);
    ensureVisible(idx);
  };

  useEffect(() => {
    if (!pendingFocus.current) return;
    pendingFocus.current = false;
    const el = [...(scrollRef.current?.querySelectorAll<HTMLElement>("[role=treeitem][data-rel]") ?? [])].find((n) => n.dataset.rel === focusRel);
    el?.focus();
  });

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
    const cur = tabStop;
    if (cur === null) return;
    const i = navRows.findIndex((r) => r.e.rel === cur);
    const row = navRows[i];
    if (!row) return;
    switch (ev.key) {
      case "ArrowDown": ev.preventDefault(); if (i < navRows.length - 1) moveFocus(navRows[i + 1].e.rel); break;
      case "ArrowUp": ev.preventDefault(); if (i > 0) moveFocus(navRows[i - 1].e.rel); break;
      case "Home": ev.preventDefault(); moveFocus(navRows[0].e.rel); break;
      case "End": ev.preventDefault(); moveFocus(navRows[navRows.length - 1].e.rel); break;
      case "ArrowRight":
        ev.preventDefault();
        if (row.e.isDir) {
          if (!expanded.has(row.e.rel)) toggle(row.e.rel);
          else if (navRows[i + 1] && parentOf(navRows[i + 1].e.rel) === row.e.rel) moveFocus(navRows[i + 1].e.rel);
        }
        break;
      case "ArrowLeft":
        ev.preventDefault();
        if (row.e.isDir && expanded.has(row.e.rel)) toggle(row.e.rel);
        else if (parentOf(row.e.rel) !== "" || row.level > 1) {
          const p = parentOf(row.e.rel);
          if (rowIndex.has(p)) moveFocus(p);
        }
        break;
      case "Enter":
      case " ":
        ev.preventDefault();
        if (row.e.isDir) toggle(row.e.rel);
        else openEntry(row.e);
        break;
      default:
    }
  };

  // ---- opening a folder ----
  const doOpen = async (path: string) => {
    setOpenErr(null);
    try {
      const w = await openFolder(path);
      setWorkspace(w);
    } catch (e) {
      setOpenErr(errMsg(e));
    }
  };
  const chooseFolder = async () => {
    setOpenErr(null);
    let path: string | null;
    try {
      path = await pickFolder();
    } catch (e) {
      setOpenErr(errCode(e) === "no_tauri" ? "Opening a folder needs the desktop app." : errMsg(e));
      return;
    }
    if (path) await requestOpen(path);
  };
  const requestOpen = async (path: string) => {
    if (Object.values(models.getState().buffers).some((b) => b.dirty)) setConfirmPath(path);
    else await doOpen(path);
  };

  const start = Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN);
  const end = Math.min(rows.length, Math.ceil((scrollTop + viewH) / rowH) + OVERSCAN);
  const indices: number[] = [];
  for (let i = start; i < end; i++) indices.push(i);
  const fi = tabStop !== null ? rowIndex.get(tabStop) : undefined;
  if (fi !== undefined && (fi < start || fi >= end)) indices.push(fi); // keep the roving tab stop in the DOM

  const rootState = dirs[""];

  return (
    <div className="filetree">
      <div className="ft-head">
        <h2>{info ? info.name.toUpperCase() : "FILES"}</h2>
        <button type="button" className="ft-btn" aria-label="Open folder" title="Open folder" onClick={() => void chooseFolder()}>&#128193;</button>
      </div>
      {openErr && <p className="ft-err" role="alert">{openErr}</p>}
      {root === null ? (
        <div className="ft-empty">
          <p>No folder open.</p>
          <button type="button" className="primary" onClick={() => void chooseFolder()}>Open folder</button>
          {recent && recent.length > 0 && (
            <ul className="ft-recent" aria-label="Recent folders">
              {recent.slice(0, 5).map((r) => (
                <li key={r}><button type="button" title={r} onClick={() => void requestOpen(r)}>{r.split(/[\\/]/).filter(Boolean).pop() ?? r}</button></li>
              ))}
            </ul>
          )}
        </div>
      ) : (
        <>
          {rootState?.status === "error" && <p className="ft-err" role="alert">Couldn&apos;t read the folder: {rootState.error} <button type="button" onClick={() => void load("", gen.current)}>Retry</button></p>}
          {rootState?.status === "ok" && rootState.entries.length === 0 && <p className="ft-note">This folder is empty.</p>}
          <div
            ref={scrollRef}
            className="ft-scroll"
            onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          >
            <div
              role="tree"
              aria-label="File tree"
              className="ft-tree"
              style={{ height: rows.length * rowH }}
              onKeyDown={onKeyDown}
            >
              {indices.map((i) => {
                const r = rows[i];
                const style = { top: i * rowH, height: rowH, paddingLeft: `${0.375 + (r.kind === "entry" ? r.level : r.level) * 0.75}rem` };
                if (r.kind === "msg") {
                  return <div key={r.key} role="treeitem" aria-disabled="true" aria-level={r.level} className="ft-row msg" style={style}>{r.text}</div>;
                }
                const { e } = r;
                const isOpen = e.isDir && expanded.has(e.rel);
                const sel = !e.isDir && editor.active === e.rel;
                return (
                  <div
                    key={e.rel}
                    role="treeitem"
                    data-rel={e.rel}
                    aria-level={r.level}
                    aria-posinset={r.pos}
                    aria-setsize={r.size}
                    aria-expanded={e.isDir ? isOpen : undefined}
                    aria-selected={sel}
                    tabIndex={e.rel === tabStop ? 0 : -1}
                    className={`ft-row${e.isDir ? " dir" : ""}${sel ? " sel" : ""}`}
                    style={style}
                    onClick={() => {
                      setFocusRel(e.rel);
                      if (e.isDir) toggle(e.rel);
                      else openEntry(e);
                    }}
                  >
                    <span className="ft-chev" aria-hidden="true">{e.isDir ? (isOpen ? "▾" : "▸") : ""}</span>
                    <span className="ft-name">{e.name}</span>
                  </div>
                );
              })}
            </div>
          </div>
        </>
      )}
      {confirmPath && (
        <ConfirmDialog
          title="Discard unsaved changes?"
          message="Some open files have unsaved changes. Opening another folder will discard them."
          actions={[
            { label: "Cancel", value: "cancel", primary: true }, // safe default focus
            { label: "Discard and open", value: "open" },
          ]}
          onChoose={(v) => {
            const p = confirmPath;
            setConfirmPath(null);
            if (v === "open") void doOpen(p);
          }}
        />
      )}
    </div>
  );
}
