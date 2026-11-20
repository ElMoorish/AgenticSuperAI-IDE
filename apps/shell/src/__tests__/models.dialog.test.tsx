import { cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { aliasFor, describeSuggestion, fmtSize, validateLaunch } from "../models/modelsLib";

afterEach(cleanup);

const ok = { port: "8080", ctx: "16384", ngl: "99", moe: "0", alias: "m" };

describe("models helpers", () => {
  it("formats sizes like a file manager", () => {
    expect(fmtSize(23.4e9)).toBe("23.4 GB");
    expect(fmtSize(512e6)).toBe("512 MB");
    expect(fmtSize(0)).toBe("0 MB");
    expect(fmtSize(10)).toBe("1 MB");
  });

  it("accepts a good form and names the first problem otherwise", () => {
    expect(validateLaunch(ok)).toBeNull();
    expect(validateLaunch({ ...ok, port: "80" })).toMatch(/Port/);
    expect(validateLaunch({ ...ok, port: "abc" })).toMatch(/Port/);
    expect(validateLaunch({ ...ok, ctx: "100" })).toMatch(/Context/);
    expect(validateLaunch({ ...ok, ngl: "-1" })).toMatch(/GPU layers/);
    expect(validateLaunch({ ...ok, moe: "1.5" })).toMatch(/Expert/);
    expect(validateLaunch({ ...ok, alias: "  " })).toMatch(/name/);
  });

  it("explains the suggestion in plain words", () => {
    expect(describeSuggestion({ ngl: 99, n_cpu_moe: 0, fits_gpu: true }, 8188)).toMatch(/Fits/);
    expect(describeSuggestion({ ngl: 99, n_cpu_moe: 28, fits_gpu: false }, 8188)).toMatch(/28 layers/);
    expect(describeSuggestion({ ngl: 30, n_cpu_moe: 0, fits_gpu: false }, 8188)).toMatch(/30 layers run on the GPU/);
    expect(describeSuggestion({ ngl: 0, n_cpu_moe: 0, fits_gpu: false }, null)).toMatch(/CPU/);
  });

  it("alias keeps only safe characters", () => {
    expect(aliasFor({ name: "Qwen 3.6 (Q4)", path: "", size_bytes: 1, quant: null, source: "folder", mmproj: null })).toBe("Qwen-3.6-Q4");
    expect(aliasFor({ name: "***", path: "", size_bytes: 1, quant: null, source: "folder", mmproj: null })).toBe("model");
  });
});
