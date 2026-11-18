import { CHOSEN_KEY, STORAGE_KEY, clampZoom, useStore } from "./state/store";
import { applyTheme, isThemeName, resolveInitialTheme, type ThemeName } from "./styles/themes";

/**
 * Runs synchronously before React renders: applies the last known theme and zoom from
 * localStorage so there is no flash of the wrong theme. The async settings load reconciles later.
 */
export function bootstrapTheme(root: HTMLElement = document.documentElement): ThemeName {
  let chosen: string | null = null;
  let zoom = 1;
  try {
    chosen = localStorage.getItem(CHOSEN_KEY);
  } catch {
    /* storage unavailable */
  }
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (raw && typeof raw.zoom === "number") zoom = clampZoom(raw.zoom);
  } catch {
    /* corrupt: default zoom */
  }
  const prefersLight = typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: light)").matches;
  const theme = resolveInitialTheme(isThemeName(chosen) ? chosen : null, isThemeName(chosen), prefersLight);
  applyTheme(theme, root);
  root.style.setProperty("--zoom", String(zoom));
  useStore.setState({ theme });
  return theme;
}
