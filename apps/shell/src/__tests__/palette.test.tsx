import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { handleShortcut } from "../App";
import { models } from "../editor/models";
import { CommandPalette } from "../palette/CommandPalette";
import { usePalette } from "../palette/paletteStore";
import { useSettingsDialog } from "../settings/dialogStore";
import { useStore } from "../state/store";
import { TitleBar } from "../layout/TitleBar";

const search = vi.fn();
vi.mock("../ipc", async (orig) => ({
  ...(await orig<typeof import("../ipc")>()),
  searchFiles: (q: string) => search(q),
  probeEndpoint: async () => ({ ok: false, models: [], error: "x" }),
  windowControl: async () => {},
}));

beforeEach(() => {
  search.mockReset();
  usePalette.setState({ isOpen: false });
  useSettingsDialog.setState({ isOpen: false });
  useStore.setState({ workspace: { info: { root: "C:/p", name: "p" } } });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("command palette", () => {
  it("finds files by name and opens the chosen one with Enter", async () => {
    search.mockResolvedValue(["src/main.lua", "main.cfg"]);
    const open = vi.spyOn(models, "openFile").mockResolvedValue(undefined as never);
    render(<CommandPalette />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "main" } });
    await screen.findByText("main.lua");
    expect(search).toHaveBeenCalledWith("main");
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "ArrowDown" });
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Enter" });
    expect(open).toHaveBeenCalledWith("main.cfg");
    expect(usePalette.getState().isOpen).toBe(false);
  });

  it("> lists commands and runs one", () => {
    render(<CommandPalette />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: ">settings" } });
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Enter" });
    expect(useSettingsDialog.getState().isOpen).toBe(true);
    expect(search).not.toHaveBeenCalled();
  });

  it("says what to do when no folder is open, and shows host errors", async () => {
    useStore.setState({ workspace: { info: null } });
    render(<CommandPalette />);
    expect(screen.getByRole("status").textContent).toMatch(/Open a folder/);
    cleanup();
    useStore.setState({ workspace: { info: { root: "C:/p", name: "p" } } });
    search.mockRejectedValue(new Error("boom"));
    render(<CommandPalette />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "x" } });
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("boom"));
  });

  it("Escape closes it", () => {
    usePalette.setState({ isOpen: true });
    render(<CommandPalette />);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(usePalette.getState().isOpen).toBe(false);
  });
});

describe("opening the palette", () => {
  it("the title bar search box opens it", () => {
    render(<TitleBar />);
    fireEvent.click(screen.getByRole("button", { name: /Search files and commands/ }));
    expect(usePalette.getState().isOpen).toBe(true);
  });

  it("Ctrl+K and Ctrl+P toggle it; Ctrl+P also works inside the editor", () => {
    const ev = (key: string, target?: Element) => {
      const e = new KeyboardEvent("keydown", { key, ctrlKey: true, bubbles: true, cancelable: true });
      (target ?? document.body).dispatchEvent(e);
      return e;
    };
    expect(handleShortcut(ev("k"))).toBe(true);
    expect(usePalette.getState().isOpen).toBe(true);
    expect(handleShortcut(ev("k"))).toBe(true);
    expect(usePalette.getState().isOpen).toBe(false);
    const host = document.createElement("div");
    host.className = "monaco-editor";
    document.body.appendChild(host);
    expect(handleShortcut(ev("k", host))).toBe(false); // Monaco keeps Ctrl+K for its chords
    expect(handleShortcut(ev("p", host))).toBe(true);
    expect(usePalette.getState().isOpen).toBe(true);
    host.remove();
  });
});
