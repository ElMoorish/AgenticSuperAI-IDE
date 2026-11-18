import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "../editor/ConfirmDialog";
import type { Buffer } from "../editor/models";
import { tabLabel, Tabs } from "../editor/Tabs";
import * as ipc from "../ipc";

afterEach(cleanup);

const b = (rel: string, over: Partial<Buffer> = {}): Buffer => ({
  rel, status: "ready", dirty: false, stamp: null, viewState: undefined, readOnly: false, language: "plaintext",
  conflict: null, deleted: false, canOpenAnyway: false, ...over,
});

function setup(over: Partial<Record<string, Partial<Buffer>>> = {}) {
  const order = ["a.ts", "src/b.ts", "docs/b.ts"];
  const buffers = Object.fromEntries(order.map((r) => [r, b(r, over[r])]));
  const h = { onActivate: vi.fn(), onClose: vi.fn(), onSave: vi.fn() };
  render(<Tabs order={order} buffers={buffers} active="a.ts" {...h} />);
  return h;
}

describe("Tabs", () => {
  it("exposes tablist/tab roles with roving tabindex and disambiguated names", () => {
    setup({ "a.ts": { dirty: true } });
    expect(screen.getByRole("tablist")).toBeTruthy();
    const tabs = screen.getAllByRole("tab");
    expect(tabs).toHaveLength(3);
    expect(tabs.map((t) => t.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    expect(screen.getByLabelText("unsaved changes")).toBeTruthy();
    expect(tabLabel("src/b.ts", ["a.ts", "src/b.ts", "docs/b.ts"])).toBe("b.ts · src");
    expect(tabLabel("a.ts", ["a.ts", "src/b.ts"])).toBe("a.ts");
  });

  it("arrow keys, Home/End, Ctrl+W, Ctrl+S and middle click work", () => {
    const h = setup();
    const [t0] = screen.getAllByRole("tab");
    fireEvent.keyDown(t0, { key: "ArrowRight" });
    expect(h.onActivate).toHaveBeenLastCalledWith("src/b.ts");
    fireEvent.keyDown(t0, { key: "ArrowLeft" });
    expect(h.onActivate).toHaveBeenLastCalledWith("docs/b.ts"); // wraps
    fireEvent.keyDown(t0, { key: "End" });
    expect(h.onActivate).toHaveBeenLastCalledWith("docs/b.ts");
    fireEvent.keyDown(t0, { key: "w", ctrlKey: true });
    expect(h.onClose).toHaveBeenLastCalledWith("a.ts");
    fireEvent.keyDown(t0, { key: "s", ctrlKey: true });
    expect(h.onSave).toHaveBeenLastCalledWith("a.ts");
    fireEvent(screen.getAllByRole("tab")[1], new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(h.onClose).toHaveBeenLastCalledWith("src/b.ts");
    fireEvent.click(screen.getByLabelText("Close a.ts"));
    expect(h.onClose).toHaveBeenLastCalledWith("a.ts");
  });

  it("marks a deleted file", () => {
    setup({ "a.ts": { deleted: true } });
    expect(screen.getByText("deleted")).toBeTruthy();
  });
});

describe("ConfirmDialog", () => {
  it("is an in-app alertdialog: focuses the primary action, Esc cancels, Tab stays inside", () => {
    const onChoose = vi.fn();
    render(<ConfirmDialog title="Save?" message="m" actions={[{ label: "Save", value: "save", primary: true }, { label: "Cancel", value: "cancel" }]} onChoose={onChoose} />);
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Save");
    fireEvent.keyDown(document.activeElement!, { key: "Tab" });
    expect(document.activeElement?.textContent).toBe("Cancel");
    fireEvent.keyDown(document.activeElement!, { key: "Tab" });
    expect(document.activeElement?.textContent).toBe("Save");
    fireEvent.click(screen.getByText("Cancel"));
    expect(onChoose).toHaveBeenLastCalledWith("cancel");
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    expect(onChoose).toHaveBeenLastCalledWith(null);
  });
});

describe("ipc channels outside Tauri", () => {
  it("subscribeEvents rejects with no_tauri instead of a TypeError", async () => {
    await expect(ipc.subscribeEvents(() => {})).rejects.toMatchObject({ code: "no_tauri" });
    await expect(ipc.agentPrompt("plan", "x", () => {})).rejects.toMatchObject({ code: "no_tauri" });
  });
});
