import type { ITheme } from "@xterm/xterm";

/** xterm colours taken from the live theme variables, so the terminal matches whichever theme is active. */
export function terminalTheme(root: HTMLElement = document.documentElement): ITheme {
  const v = (n: string, d: string) => getComputedStyle(root).getPropertyValue(`--${n}`).trim() || d;
  return {
    background: v("bg", "#000000"),
    foreground: v("text", "#dddddd"),
    cursor: v("accent", "#ffffff"),
    cursorAccent: v("bg", "#000000"),
    selectionBackground: v("raise", "#333333"),
    black: v("chrome", "#000000"),
    red: v("err", "#ff6b6b"),
    green: v("ok", "#58d6a0"),
    yellow: v("warn", "#f2b84b"),
    blue: v("fn", "#7cc7ff"),
    magenta: v("kw", "#c3a6ff"),
    cyan: v("accent-2", "#2de2ff"),
    white: v("text", "#dddddd"),
    brightBlack: v("muted", "#888888"),
    brightRed: v("err", "#ff6b6b"),
    brightGreen: v("ok", "#58d6a0"),
    brightYellow: v("nu", "#ffb27a"),
    brightBlue: v("fn", "#7cc7ff"),
    brightMagenta: v("kw", "#c3a6ff"),
    brightCyan: v("accent-2", "#2de2ff"),
    brightWhite: v("text", "#ffffff"),
  };
}
