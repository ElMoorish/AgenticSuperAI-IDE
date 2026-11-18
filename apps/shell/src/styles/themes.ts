/**
 * Six themes. Values are taken from the approved workbench mockup
 * (mockup --dim is exposed as `muted`, --acc as `accent`, --acc-ink as `accent-ink`).
 * Tokens the mockup leaves undefined for a theme (warn/err for graphite, aurora, phosphor and
 * ember; the mockup CSS inherits them from harbor) are inherited from harbor here too.
 * `focus-ring` is new (not in the mockup): each theme uses its accent.
 */
export const THEME_NAMES = ["harbor", "paper", "graphite", "aurora", "phosphor", "ember"] as const;
export type ThemeName = (typeof THEME_NAMES)[number];

export const TOKEN_NAMES = [
  "bg", "chrome", "panel", "raise", "line", "text", "muted",
  "accent", "accent-ink", "ok", "warn", "err", "add", "del",
  "kw", "st", "fn", "nu", "cm", "focus-ring",
] as const;
export type TokenName = (typeof TOKEN_NAMES)[number];
export type ThemeTokens = Record<TokenName, string>;

export const THEME_LABELS: Record<ThemeName, string> = {
  harbor: "Harbor",
  paper: "Paper (light)",
  graphite: "Graphite",
  aurora: "Aurora",
  phosphor: "Phosphor",
  ember: "Ember",
};

export function isThemeName(v: unknown): v is ThemeName {
  return typeof v === "string" && (THEME_NAMES as readonly string[]).includes(v);
}

const harbor: ThemeTokens = {
  bg: "#0e1621", chrome: "#0a111a", panel: "#111c29", raise: "#16243a", line: "#223148",
  text: "#dbe6f5", muted: "#7d8fa8", accent: "#5eb6ff", "accent-ink": "#04202f",
  ok: "#58d6a0", warn: "#f2b84b", err: "#ff6b6b", add: "#0f3a2b", del: "#43202a",
  kw: "#c3a6ff", st: "#8fe3a8", fn: "#7cc7ff", nu: "#ffb27a", cm: "#5d7089",
  "focus-ring": "#5eb6ff",
};

export const themes: Record<ThemeName, ThemeTokens> = {
  harbor,
  graphite: {
    ...harbor,
    bg: "#1b1b1d", chrome: "#141416", panel: "#212124", raise: "#2a2a2e", line: "#35353a",
    text: "#e6e6ea", muted: "#8d8d96", accent: "#ff8a3d", "accent-ink": "#2a1200",
    ok: "#7ed492", add: "#1d3a25", del: "#41232a",
    kw: "#ff8a3d", st: "#9ece6a", fn: "#7aa2f7", nu: "#e0af68", cm: "#66666e",
    "focus-ring": "#ff8a3d",
  },
  aurora: {
    ...harbor,
    bg: "#120f20", chrome: "#0d0b18", panel: "#181430", raise: "#211c40", line: "#2f2956",
    text: "#ebe7ff", muted: "#8f88bd", accent: "#a78bff", "accent-ink": "#140b33",
    ok: "#3ddc97", add: "#123a2e", del: "#41203a",
    kw: "#c8b0ff", st: "#3ddc97", fn: "#7cc4ff", nu: "#ffb86b", cm: "#625b8f",
    "focus-ring": "#a78bff",
  },
  paper: {
    ...harbor,
    bg: "#fdfbf7", chrome: "#f1ece3", panel: "#f7f3ec", raise: "#ffffff", line: "#ddd5c7",
    text: "#2b2926", muted: "#837c70", accent: "#b4380d", "accent-ink": "#ffffff",
    ok: "#15803d", warn: "#b45309", err: "#c62828", add: "#d9f3e1", del: "#fbdcdc",
    kw: "#7c3aed", st: "#15803d", fn: "#1d4ed8", nu: "#b45309", cm: "#a39c8f",
    "focus-ring": "#b4380d",
  },
  phosphor: {
    ...harbor,
    bg: "#050b06", chrome: "#030704", panel: "#09130a", raise: "#0e1d10", line: "#173a1c",
    text: "#bff5c4", muted: "#53905a", accent: "#3dff74", "accent-ink": "#021407",
    ok: "#3dff74", add: "#0c3a18", del: "#3b1212",
    kw: "#3dff74", st: "#eaff6b", fn: "#6bffdb", nu: "#ffb86b", cm: "#3b6b41",
    "focus-ring": "#3dff74",
  },
  ember: {
    ...harbor,
    bg: "#1a1211", chrome: "#130d0c", panel: "#221716", raise: "#2c1d1b", line: "#3d2724",
    text: "#f4e6e1", muted: "#a38a84", accent: "#ff6a55", "accent-ink": "#2a0804",
    ok: "#7ed492", add: "#183a22", del: "#44191b",
    kw: "#ff8a7a", st: "#ffb454", fn: "#6fc3ff", nu: "#f0a1ff", cm: "#6e5650",
    "focus-ring": "#ff6a55",
  },
};

/** Set every token as a CSS custom property (`--bg`, `--focus-ring`, ...) on <html>. */
export function applyTheme(name: ThemeName, root: HTMLElement = document.documentElement): void {
  const t = themes[name];
  for (const tok of TOKEN_NAMES) root.style.setProperty(`--${tok}`, t[tok]);
  root.dataset.theme = name;
  root.style.colorScheme = name === "paper" ? "light" : "dark";
}

/**
 * Initial theme: a theme the user chose wins; otherwise follow the OS (`light` -> paper).
 * `settingsTheme` defaults to "harbor" on the host, so harbor only counts as a choice when
 * `chosen` (the user explicitly picked a theme once) is true.
 */
export function resolveInitialTheme(settingsTheme: unknown, chosen: boolean, prefersLight: boolean): ThemeName {
  if (isThemeName(settingsTheme) && (settingsTheme !== "harbor" || chosen)) return settingsTheme;
  return prefersLight ? "paper" : "harbor";
}
