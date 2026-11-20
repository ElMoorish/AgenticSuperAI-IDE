/**
 * Pure theme data: one Monaco theme per app theme, derived from themes.ts tokens.
 * No Monaco import here, so it is unit-testable and keeps Monaco out of the initial bundle.
 */
import { THEME_NAMES, themeStyles, themes, type ThemeName } from "../styles/themes";

export interface MonacoThemeData {
  base: "vs" | "vs-dark";
  inherit: boolean;
  rules: { token: string; foreground?: string; fontStyle?: string }[];
  colors: Record<string, string>;
}

export const monacoThemeName = (t: ThemeName): string => `asa-${t}`;
export const MONACO_THEME_NAMES: string[] = THEME_NAMES.map(monacoThemeName);

const hex = (c: string): string => c.replace("#", "").toLowerCase();

export function buildMonacoTheme(name: ThemeName): MonacoThemeData {
  const t = themes[name];
  const kw = hex(t.kw), st = hex(t.st), fn = hex(t.fn), nu = hex(t.nu), cm = hex(t.cm);
  return {
    base: themeStyles[name].light ? "vs" : "vs-dark",
    inherit: true,
    rules: [
      { token: "", foreground: hex(t.text) },
      { token: "keyword", foreground: kw },
      { token: "keyword.control", foreground: kw },
      { token: "storage", foreground: kw },
      { token: "string", foreground: st },
      { token: "string.escape", foreground: nu },
      { token: "regexp", foreground: st },
      { token: "number", foreground: nu },
      { token: "constant", foreground: nu },
      { token: "comment", foreground: cm, fontStyle: "italic" },
      { token: "type", foreground: fn },
      { token: "type.identifier", foreground: fn },
      { token: "function", foreground: fn },
      { token: "tag", foreground: kw },
      { token: "attribute.name", foreground: fn },
      { token: "attribute.value", foreground: st },
      { token: "delimiter", foreground: hex(t.muted) },
      { token: "string.key.json", foreground: fn },
      { token: "string.value.json", foreground: st },
    ],
    colors: {
      "editor.background": t.bg,
      "editor.foreground": t.text,
      "editorLineNumber.foreground": t.muted,
      "editorLineNumber.activeForeground": t.text,
      "editor.lineHighlightBackground": t.raise,
      "editor.lineHighlightBorder": t.raise,
      "editor.selectionBackground": t.line,
      "editor.inactiveSelectionBackground": t.raise,
      "editorCursor.foreground": t.accent,
      "editorWhitespace.foreground": t.line,
      "editorIndentGuide.background1": t.line,
      "editorIndentGuide.activeBackground1": t.muted,
      "editorWidget.background": t.panel,
      "editorWidget.border": t.line,
      "editorSuggestWidget.background": t.panel,
      "editorSuggestWidget.border": t.line,
      "editorSuggestWidget.selectedBackground": t.raise,
      "editorHoverWidget.background": t.panel,
      "editorHoverWidget.border": t.line,
      "editorGutter.background": t.bg,
      "diffEditor.insertedLineBackground": t.add,
      "diffEditor.removedLineBackground": t.del,
      "diffEditor.insertedTextBackground": t.add,
      "diffEditor.removedTextBackground": t.del,
      "scrollbarSlider.background": t.line + "99",
      "scrollbarSlider.hoverBackground": t.muted + "88",
      "scrollbarSlider.activeBackground": t.muted + "bb",
      focusBorder: t.accent,
    },
  };
}
