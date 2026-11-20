import { create } from "zustand";
import { getSettings, isTauri, setSettings, type Settings, type WorkspaceInfo } from "../ipc";
import { DEFAULT_THEME, applyTheme, resolveInitialTheme, type ThemeName } from "../styles/themes";

export type PanelKey = "filePanel" | "agent" | "bottom";

/** Default sizes (px at zoom 1) come from the approved mockup: 186 / 318 / 206. */
export const DEFAULT_SIZES: Record<PanelKey, number> = { filePanel: 186, agent: 318, bottom: 206 };
export const LIMITS: Record<PanelKey, { min: number; max: number }> = {
  filePanel: { min: 140, max: 480 },
  agent: { min: 260, max: 640 },
  bottom: { min: 80, max: 600 },
};
export const ZOOM_MIN = 0.8;
export const ZOOM_MAX = 1.6;
export const ZOOM_STEP = 0.1;

export function clampSize(key: PanelKey, v: number): number {
  const { min, max } = LIMITS[key];
  if (!Number.isFinite(v)) return min;
  return Math.min(max, Math.max(min, Math.round(v)));
}

export function clampZoom(z: number): number {
  if (!Number.isFinite(z)) return 1;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(z * 100) / 100));
}

export interface LayoutState {
  sizes: Record<PanelKey, number>;
  collapsed: Record<PanelKey, boolean>;
}

export const STORAGE_KEY = "asa.ui.v1";
export const CHOSEN_KEY = "asa.theme.chosen";

const freshLayout = (): LayoutState => ({
  sizes: { ...DEFAULT_SIZES },
  collapsed: { filePanel: false, agent: false, bottom: false },
});

// Layout and zoom are persisted to localStorage: ide-host's Settings has no layout field
// (and Rust rejects unknown keys), so this avoids a Rust change.
function loadPersisted(): { layout: LayoutState; zoom: number } {
  const base = { layout: freshLayout(), zoom: 1 };
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (!raw || typeof raw !== "object") return base;
    for (const k of Object.keys(DEFAULT_SIZES) as PanelKey[]) {
      const sz = raw.layout?.sizes?.[k];
      if (typeof sz === "number") base.layout.sizes[k] = clampSize(k, sz);
      base.layout.collapsed[k] = raw.layout?.collapsed?.[k] === true;
    }
    if (typeof raw.zoom === "number") base.zoom = clampZoom(raw.zoom);
  } catch {
    /* storage unavailable or corrupt: defaults */
  }
  return base;
}

function persist(layout: LayoutState, zoom: number): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ layout, zoom }));
  } catch {
    /* ignore */
  }
}

export interface StoreState {
  workspace: { info: WorkspaceInfo | null };
  settings: { value: Settings | null; loaded: boolean; error: string | null };
  theme: ThemeName;
  zoom: number;
  layout: LayoutState;

  setWorkspace: (info: WorkspaceInfo | null) => void;
  setSize: (key: PanelKey, px: number) => void;
  toggleCollapsed: (key: PanelKey, value?: boolean) => void;
  resetLayout: () => void;
  setZoom: (z: number) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomReset: () => void;
  /** Applies the theme immediately and saves it through settings (best effort). */
  setTheme: (name: ThemeName) => Promise<void>;
  /** Load settings via ipc (graceful outside Tauri) and apply the initial theme. */
  boot: () => Promise<void>;
}

const persisted = loadPersisted();

export const useStore = create<StoreState>((set, get) => ({
  workspace: { info: null },
  settings: { value: null, loaded: false, error: null },
  theme: DEFAULT_THEME,
  zoom: persisted.zoom,
  layout: persisted.layout,

  setWorkspace: (info) => set({ workspace: { info } }),

  setSize: (key, px) => {
    const cur = get().layout;
    // resizing a collapsed panel (drag or arrow key on its splitter) re-opens it
    const layout = { sizes: { ...cur.sizes, [key]: clampSize(key, px) }, collapsed: { ...cur.collapsed, [key]: false } };
    set({ layout });
    persist(layout, get().zoom);
  },
  toggleCollapsed: (key, value) => {
    const cur = get().layout;
    const layout = { ...cur, collapsed: { ...cur.collapsed, [key]: value ?? !cur.collapsed[key] } };
    set({ layout });
    persist(layout, get().zoom);
  },
  resetLayout: () => {
    const layout = freshLayout();
    set({ layout });
    persist(layout, get().zoom);
  },

  setZoom: (z) => {
    const zoom = clampZoom(z);
    set({ zoom });
    persist(get().layout, zoom);
  },
  zoomIn: () => get().setZoom(get().zoom + ZOOM_STEP),
  zoomOut: () => get().setZoom(get().zoom - ZOOM_STEP),
  zoomReset: () => get().setZoom(1),

  setTheme: async (name) => {
    applyTheme(name);
    set({ theme: name });
    try {
      localStorage.setItem(CHOSEN_KEY, name);
    } catch {
      /* ignore */
    }
    if (isTauri()) {
      try {
        // merge onto the freshest host copy so other fields are never reverted
        const cur = await getSettings();
        const saved = await setSettings({ ...cur, theme: name });
        set({ settings: { value: saved, loaded: true, error: null } });
      } catch (e) {
        set({ settings: { ...get().settings, error: e instanceof Error ? e.message : String(e) } });
      }
    }
  },

  boot: async () => {
    let value: Settings | null = null;
    let error: string | null = null;
    try {
      value = await getSettings();
    } catch (e) {
      // Not in Tauri (plain browser / tests) or the host failed: run with defaults.
      const code = (e as { code?: string }).code;
      error = code === "no_tauri" ? null : e instanceof Error ? e.message : String(e);
    }
    let local: string | null = null;
    try {
      local = localStorage.getItem(CHOSEN_KEY);
    } catch {
      /* ignore */
    }
    const theme = resolveInitialTheme(value ? value.theme : local);
    try {
      localStorage.setItem(CHOSEN_KEY, theme); // lets bootstrapTheme avoid a flash next launch
    } catch {
      /* ignore */
    }
    applyTheme(theme);
    set({ settings: { value, loaded: true, error }, theme });
  },
}));
