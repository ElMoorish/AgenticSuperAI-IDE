import { describe, expect, it } from "vitest";
import { THEME_NAMES, TOKEN_NAMES, applyTheme, themes } from "../styles/themes";
import { contrastRatio } from "../styles/contrast";

describe("themes", () => {
  it("themes_define_every_token_for_every_theme", () => {
    expect([...THEME_NAMES].sort()).toEqual(["aurora", "deepsea", "drafting", "forge", "monolith", "voltage"]);
    for (const name of THEME_NAMES) {
      for (const tok of TOKEN_NAMES) {
        const v = themes[name][tok];
        expect(v, `${name}.${tok}`).toMatch(/^#[0-9a-f]{6}$/i);
      }
      expect(Object.keys(themes[name]).sort()).toEqual([...TOKEN_NAMES].sort());
    }
  });

  it("applyTheme_sets_variables", () => {
    for (const name of THEME_NAMES) {
      applyTheme(name);
      const root = document.documentElement;
      for (const tok of TOKEN_NAMES) {
        expect(root.style.getPropertyValue(`--${tok}`)).toBe(themes[name][tok]);
      }
      expect(root.dataset.theme).toBe(name);
    }
  });

  it("focus_ring_token_defined_in_every_theme", () => {
    for (const name of THEME_NAMES) {
      expect(themes[name]["focus-ring"], name).toMatch(/^#[0-9a-f]{6}$/i);
      // the ring must be visible against the surfaces it is drawn on
      expect(contrastRatio(themes[name]["focus-ring"], themes[name].bg), `${name} ring/bg`).toBeGreaterThanOrEqual(3);
      expect(contrastRatio(themes[name]["focus-ring"], themes[name].panel), `${name} ring/panel`).toBeGreaterThanOrEqual(3);
    }
  });

  it("contrast_meets_AA_for_text_on_bg_in_every_theme", () => {
    for (const name of THEME_NAMES) {
      const t = themes[name];
      expect(contrastRatio(t.text, t.bg), `${name} text/bg`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(t.text, t.panel), `${name} text/panel`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(t.muted, t.panel), `${name} muted/panel`).toBeGreaterThanOrEqual(3);
      expect(contrastRatio(t.accent, t.bg), `${name} accent/bg`).toBeGreaterThanOrEqual(3);
      expect(contrastRatio(t["accent-ink"], t.accent), `${name} accent-ink/accent`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("contrast helper matches WCAG reference values", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 1);
  });
});

describe("themes AA text (fix round 1)", () => {
  it("muted_meets_AA_on_every_surface", () => {
    for (const name of THEME_NAMES) {
      const t = themes[name];
      for (const surf of ["bg", "chrome", "panel", "raise"] as const) {
        expect(contrastRatio(t.muted, t[surf]), `${name} muted/${surf}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
  it("on_err_text_meets_AA_on_err", () => {
    for (const name of THEME_NAMES) {
      const t = themes[name];
      expect(t["on-err"], name).toMatch(/^#[0-9a-f]{6}$/i);
      expect(contrastRatio(t["on-err"], t.err), `${name} on-err/err`).toBeGreaterThanOrEqual(4.5);
    }
  });
});
