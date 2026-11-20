/**
 * Six themes. Voltage is the primary look and the default; the other five are options in Settings.
 * Values come from the approved design mockup (docs/design/mockups/themes.html). Every theme defines
 * every token (checked by tests, including WCAG AA contrast for text, muted text, accent and focus ring).
 *
 * Besides colour, a theme carries a little personality: corner radius and its own UI and code fonts
 * (bundled with the app, nothing is fetched at run time).
 */
export const THEME_NAMES = ["voltage", "forge", "aurora", "drafting", "deepsea", "monolith"] as const;
export type ThemeName = (typeof THEME_NAMES)[number];
export const DEFAULT_THEME: ThemeName = "voltage";

export const TOKEN_NAMES = [
  "bg", "chrome", "panel", "raise", "line", "text", "muted",
  "accent", "accent-ink", "accent-2", "ok", "warn", "err", "add", "del",
  "kw", "st", "fn", "nu", "cm", "on-err", "focus-ring",
] as const;
export type TokenName = (typeof TOKEN_NAMES)[number];
export type ThemeTokens = Record<TokenName, string>;

export interface ThemeStyle {
  /** Corner radius for controls and cards. */
  radius: string;
  fontUi: string;
  fontMono: string;
  /** Light themes switch the browser's colour scheme (scrollbars, form controls). */
  light: boolean;
  /** One-line character, shown in Settings. */
  blurb: string;
}

export const THEME_LABELS: Record<ThemeName, string> = {
  voltage: "Voltage",
  forge: "Forge",
  aurora: "Aurora",
  drafting: "Drafting table (light)",
  deepsea: "Deep sea",
  monolith: "Monolith",
};

export function isThemeName(v: unknown): v is ThemeName {
  return typeof v === "string" && (THEME_NAMES as readonly string[]).includes(v);
}

const sans = (name: string) => `"${name}", "Segoe UI", system-ui, sans-serif`;
const mono = (name: string) => `"${name}", Consolas, monospace`;

export const themeStyles: Record<ThemeName, ThemeStyle> = {
  voltage: { radius: "6px", fontUi: sans("Archivo Variable"), fontMono: mono("JetBrains Mono Variable"), light: false, blurb: "Electric magenta and cyan on deep indigo. Energetic, sharp, heavy type." },
  forge: { radius: "3px", fontUi: sans("Bricolage Grotesque Variable"), fontMono: mono("IBM Plex Mono"), light: false, blurb: "Hot metal. Every token is a spark struck from steel. Warm and robust." },
  aurora: { radius: "12px", fontUi: sans("Outfit Variable"), fontMono: mono("Fira Code Variable"), light: false, blurb: "Northern-lights calm. Soft, rounded, quietly energetic." },
  drafting: { radius: "2px", fontUi: sans("Hanken Grotesk Variable"), fontMono: mono("DM Mono"), light: true, blurb: "Light, ink and one signal orange. Precise like a technical drawing." },
  deepsea: { radius: "10px", fontUi: sans("Schibsted Grotesk Variable"), fontMono: mono("Martian Mono Variable"), light: false, blurb: "Bioluminescence in the dark. Mysterious and calm." },
  monolith: { radius: "0px", fontUi: sans("Familjen Grotesk Variable"), fontMono: mono("Red Hat Mono Variable"), light: false, blurb: "Black, white and one red. Swiss precision, no decoration." },
};

export const themes: Record<ThemeName, ThemeTokens> = {
  voltage: {
    bg: "#0b0d26", chrome: "#070819", panel: "#10133a", raise: "#191d52", line: "#2a2f78",
    text: "#eceeff", muted: "#9aa0e0", accent: "#ff3d9a", "accent-ink": "#14000a", "accent-2": "#2de2ff",
    ok: "#b6ff4d", warn: "#ffd23d", err: "#ff6b6b", add: "#0f3d4a", del: "#4a1636",
    kw: "#ff7ac0", st: "#7dffd0", fn: "#2de2ff", nu: "#ffd23d", cm: "#6b72c4",
    "on-err": "#1a0505", "focus-ring": "#2de2ff",
  },
  forge: {
    bg: "#15110f", chrome: "#0d0a09", panel: "#1d1714", raise: "#2a211c", line: "#3a2d25",
    text: "#f4e9df", muted: "#b09a89", accent: "#ff6a1a", "accent-ink": "#1a0b02", "accent-2": "#ffb23d",
    ok: "#9bd35a", warn: "#ffb23d", err: "#ff7a66", add: "#2c3a1a", del: "#4a1f12",
    kw: "#ff9a5a", st: "#e9d27a", fn: "#ffb23d", nu: "#ffd76a", cm: "#86705f",
    "on-err": "#1a0805", "focus-ring": "#ffb23d",
  },
  aurora: {
    bg: "#08140f", chrome: "#050d0a", panel: "#0d1c16", raise: "#14291f", line: "#1f3b2e",
    text: "#e3f6ec", muted: "#8bb5a2", accent: "#3dffb0", "accent-ink": "#04180f", "accent-2": "#9b7bff",
    ok: "#c8ff7a", warn: "#ffe27a", err: "#ff8aa0", add: "#0f3a2a", del: "#3d1f3a",
    kw: "#b69bff", st: "#9dffd0", fn: "#3dffb0", nu: "#ffe27a", cm: "#56806e",
    "on-err": "#1a0508", "focus-ring": "#3dffb0",
  },
  drafting: {
    bg: "#f2f4f7", chrome: "#e3e8ef", panel: "#fafbfc", raise: "#ffffff", line: "#c8d1dd",
    text: "#13203a", muted: "#4f5f7a", accent: "#cf3f0c", "accent-ink": "#ffffff", "accent-2": "#1f4fd8",
    ok: "#14803f", warn: "#8a5200", err: "#c62828", add: "#d9f2e1", del: "#fadcd6",
    kw: "#1f4fd8", st: "#14803f", fn: "#7a2fe0", nu: "#8a5200", cm: "#8593ab",
    "on-err": "#ffffff", "focus-ring": "#1f4fd8",
  },
  deepsea: {
    bg: "#061a22", chrome: "#04121a", panel: "#09232d", raise: "#0f3340", line: "#17465a",
    text: "#d9f2f7", muted: "#80b2c0", accent: "#34f0d0", "accent-ink": "#03211d", "accent-2": "#4aa8ff",
    ok: "#8ff5cf", warn: "#ffe27a", err: "#ff8a8a", add: "#0c3d3a", del: "#3d2230",
    kw: "#6bc1ff", st: "#8ff5cf", fn: "#34f0d0", nu: "#ffe27a", cm: "#4a8196",
    "on-err": "#1a0505", "focus-ring": "#34f0d0",
  },
  monolith: {
    bg: "#0c0c0c", chrome: "#050505", panel: "#121212", raise: "#1b1b1b", line: "#2c2c2c",
    text: "#f5f5f5", muted: "#a3a3a3", accent: "#ff2d2d", "accent-ink": "#140000", "accent-2": "#ffffff",
    ok: "#7ed492", warn: "#ffd23d", err: "#ff7070", add: "#1a2a1a", del: "#321414",
    kw: "#ffffff", st: "#bdbdbd", fn: "#ff6b6b", nu: "#e0e0e0", cm: "#707070",
    "on-err": "#140000", "focus-ring": "#ffffff",
  },
};

/** Set every token (`--bg`, `--accent-2`, ...) plus radius and fonts on <html>. */
export function applyTheme(name: ThemeName, root: HTMLElement = document.documentElement): void {
  const t = themes[name];
  const s = themeStyles[name];
  for (const tok of TOKEN_NAMES) root.style.setProperty(`--${tok}`, t[tok]);
  root.style.setProperty("--radius", s.radius);
  root.style.setProperty("--font-ui", s.fontUi);
  root.style.setProperty("--font-mono", s.fontMono);
  root.dataset.theme = name;
  root.style.colorScheme = s.light ? "light" : "dark";
}

/**
 * Initial theme: the saved choice if it is one of today's themes, otherwise Voltage.
 * Older saved names (harbor, paper, graphite, phosphor, ember) are not themes any more and fall back to Voltage.
 * The operating system's light or dark setting is deliberately ignored: Voltage is the primary look.
 */
export function resolveInitialTheme(saved: unknown): ThemeName {
  return isThemeName(saved) ? saved : DEFAULT_THEME;
}
