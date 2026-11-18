import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App, { handleShortcut } from "../App";
import { LIMITS, useStore } from "../state/store";
import { resolveInitialTheme } from "../styles/themes";

beforeEach(() => {
  localStorage.clear();
  useStore.getState().resetLayout();
  useStore.getState().zoomReset();
});
afterEach(cleanup);

describe("shell", () => {
  it("renders the layout regions outside Tauri", () => {
    render(<App />);
    for (const name of ["Activity bar", "Files", "Editor", "Bottom panel", "Agent"]) {
      expect(screen.getByLabelText(name)).toBeTruthy();
    }
    expect(screen.getAllByRole("separator")).toHaveLength(3);
  });

  it("splitter is keyboard resizable and exposes aria-valuenow", () => {
    render(<App />);
    const sep = screen.getByRole("separator", { name: "Resize file panel" });
    const start = Number(sep.getAttribute("aria-valuenow"));
    fireEvent.keyDown(sep, { key: "ArrowRight" });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(start + 16);
    fireEvent.keyDown(sep, { key: "ArrowLeft", shiftKey: true });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(Math.max(LIMITS.filePanel.min, start + 16 - 64));
    fireEvent.keyDown(sep, { key: "End" });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(LIMITS.filePanel.max);
    fireEvent.keyDown(sep, { key: "Enter" });
    expect(sep.getAttribute("aria-valuenow")).toBe("0");
  });

  it("agent splitter grows when moved left", () => {
    render(<App />);
    const sep = screen.getByRole("separator", { name: "Resize agent panel" });
    const start = Number(sep.getAttribute("aria-valuenow"));
    fireEvent.keyDown(sep, { key: "ArrowLeft" });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(start + 16);
  });

  it("zoom shortcuts adjust and clamp", () => {
    handleShortcut(new KeyboardEvent("keydown", { key: "=", ctrlKey: true }));
    expect(useStore.getState().zoom).toBeCloseTo(1.1);
    handleShortcut(new KeyboardEvent("keydown", { key: "0", ctrlKey: true }));
    expect(useStore.getState().zoom).toBe(1);
  });

  it("initial theme: saved choice wins, otherwise light OS -> paper", () => {
    expect(resolveInitialTheme("ember", false, true)).toBe("ember");
    expect(resolveInitialTheme("harbor", false, true)).toBe("paper");
    expect(resolveInitialTheme("harbor", true, true)).toBe("harbor");
    expect(resolveInitialTheme(null, false, false)).toBe("harbor");
  });
});
