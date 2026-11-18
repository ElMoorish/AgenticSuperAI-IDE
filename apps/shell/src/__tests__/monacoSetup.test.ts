import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { buildMonacoTheme, MONACO_THEME_NAMES, monacoThemeName } from "../editor/monacoThemes";
import { THEME_NAMES, themes } from "../styles/themes";

const src = (f: string) => readFileSync(resolve(__dirname, "../editor", f), "utf8");

describe("monacoSetup", () => {
  it("defines one Monaco theme per app theme from the themes.ts tokens", () => {
    expect(THEME_NAMES).toHaveLength(6);
    expect(MONACO_THEME_NAMES).toEqual(THEME_NAMES.map((t) => `asa-${t}`));
    for (const t of THEME_NAMES) {
      const d = buildMonacoTheme(t);
      expect(d.base).toBe(t === "paper" ? "vs" : "vs-dark");
      expect(d.colors["editor.background"]).toBe(themes[t].bg);
      expect(d.colors["editor.foreground"]).toBe(themes[t].text);
      expect(d.colors["editorCursor.foreground"]).toBe(themes[t].accent);
      const kw = d.rules.find((r) => r.token === "keyword");
      expect(kw?.foreground).toBe(themes[t].kw.replace("#", "").toLowerCase());
      for (const v of Object.values(d.colors)) expect(v).toMatch(/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/);
      expect(monacoThemeName(t)).toBe(`asa-${t}`);
    }
    // setup registers every theme and applies the right one
    const setup = src("monacoSetup.ts");
    expect(setup).toContain("defineTheme");
    expect(setup).toContain("THEME_NAMES");
    expect(setup).toContain("setTheme");
  });

  it("registers only the five workers through Vite ?worker imports, without eval", () => {
    const setup = src("monacoSetup.ts");
    const imports = [...setup.matchAll(/^import \w+ from "([^"]+\?worker)";/gm)].map((m) => m[1]);
    expect(imports).toHaveLength(5);
    expect(imports.some((i) => i.includes("editor.worker"))).toBe(true);
    for (const l of ["json", "css", "html", "typescript"]) expect(imports.some((i) => i.includes(`/${l}/`))).toBe(true);
    expect(setup).toContain("MonacoEnvironment");
    expect(setup).toContain("getWorker");
    expect(setup).not.toContain("getWorkerUrl");
    for (const f of ["monacoSetup.ts", "monacoModels.ts", "monacoThemes.ts", "models.ts", "EditorArea.tsx"]) {
      const text = src(f).replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
      expect(text, f).not.toMatch(/\beval\s*\(/);
      expect(text, f).not.toMatch(/new\s+Function\b/);
      expect(text, f).not.toMatch(/https?:\/\/(cdn|unpkg|cdnjs)/);
    }
  });

  it("keeps Monaco out of the eager bundle (only dynamic imports)", () => {
    for (const f of ["models.ts", "EditorArea.tsx", "Tabs.tsx", "monacoThemes.ts"]) {
      expect(src(f), f).not.toMatch(/^import[^;]*from "monaco-editor/m);
      expect(src(f), f).not.toMatch(/^import [^t][^;]*from "\.\/monaco(Setup|Models)"/m);
    }
    expect(src("models.ts")).toContain('import("./monacoModels")');
    expect(src("EditorArea.tsx")).toContain('import("./monacoModels")');
  });

  it("dev mock host is only reachable through the DEV-gated dynamic import", () => {
    const main = readFileSync(resolve(__dirname, "../main.tsx"), "utf8");
    expect(main).toMatch(/import\.meta\.env\.DEV && !isTauri\(\)/);
    expect(main).toContain('import("./dev/mockHost")');
    expect(main).not.toMatch(/^import[^;]*mockHost/m);
    expect(readFileSync(resolve(__dirname, "../dev/mockHost.ts"), "utf8")).toContain("ASA_DEV_MOCK_HOST");
  });
});
