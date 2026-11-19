import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", async (orig) => {
  const real = await orig<typeof import("@tauri-apps/api/core")>();
  return { ...real, invoke: (...a: unknown[]) => invokeMock(...a) };
});

import App, { shortcutAction, shouldHandleShortcut } from "../App";
import { bootstrapTheme } from "../bootstrap";
import { pickSettings, setSettings, type Settings } from "../ipc";
import { STORAGE_KEY, useStore } from "../state/store";
import { themes } from "../styles/themes";

// jsdom lacks PointerEvent coordinates: polyfill on top of MouseEvent
class PolyPointerEvent extends MouseEvent {
  pointerId: number;
  constructor(type: string, init: PointerEventInit = {}) {
    super(type, init);
    this.pointerId = init.pointerId ?? 1;
  }
}
(window as unknown as Record<string, unknown>).PointerEvent = PolyPointerEvent;

const key = (init: KeyboardEventInit, target?: Element) => {
  const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  if (target) Object.defineProperty(e, "target", { value: target });
  return e;
};

beforeEach(() => {
  localStorage.clear();
  invokeMock.mockReset();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  useStore.getState().resetLayout();
  useStore.getState().zoomReset();
});
afterEach(cleanup);

describe("shortcuts", () => {
  it("shouldHandleShortcut requires the exact modifier combo", () => {
    expect(shouldHandleShortcut(key({ key: "=", ctrlKey: true }))).toBe(true);
    expect(shouldHandleShortcut(key({ key: "+", ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(shouldHandleShortcut(key({ key: "0", ctrlKey: true }))).toBe(true);
    expect(shouldHandleShortcut(key({ key: "b", ctrlKey: true }))).toBe(true);
    expect(shortcutAction(key({ key: "b", ctrlKey: true, altKey: true }))).toBe("toggleAgent");
    // extra modifiers / no modifier / both ctrl+meta
    expect(shouldHandleShortcut(key({ key: "b", ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(shouldHandleShortcut(key({ key: "j", ctrlKey: true, altKey: true }))).toBe(false);
    expect(shouldHandleShortcut(key({ key: "0", ctrlKey: true, altKey: true }))).toBe(false);
    expect(shouldHandleShortcut(key({ key: "=" }))).toBe(false);
    expect(shouldHandleShortcut(key({ key: "b", ctrlKey: true, metaKey: true }))).toBe(false);
  });

  it("skips defaultPrevented, AltGr, and editable targets", () => {
    const prevented = key({ key: "=", ctrlKey: true });
    prevented.preventDefault();
    expect(shouldHandleShortcut(prevented)).toBe(false);

    const altgr = key({ key: "b", ctrlKey: true, altKey: true });
    altgr.getModifierState = (m: string) => m === "AltGraph";
    expect(shouldHandleShortcut(altgr)).toBe(false);

    const mk = (html: string) => {
      const host = document.createElement("div");
      host.innerHTML = html;
      document.body.appendChild(host);
      return host.firstElementChild as Element;
    };
    for (const html of [
      "<input>", "<textarea></textarea>", "<select></select>",
      "<div contenteditable='true'><span>x</span></div>",
      "<div class='xterm'><span>x</span></div>", "<div class='monaco-editor'><span>x</span></div>",
    ]) {
      const el = mk(html);
      const target = el.firstElementChild ?? el;
      expect(shouldHandleShortcut(key({ key: "=", ctrlKey: true }, target)), html).toBe(false);
    }
    expect(shouldHandleShortcut(key({ key: "=", ctrlKey: true }, document.body))).toBe(true);
  });
});

describe("splitter drag", () => {
  it("updates the CSS variable live and commits to the store only on pointerup", () => {
    const { container } = render(<App />);
    const ide = container.querySelector(".ide") as HTMLElement;
    const sep = screen.getByRole("separator", { name: "Resize file panel" });
    const start = useStore.getState().layout.sizes.filePanel;
    let storeUpdates = 0;
    const unsub = useStore.subscribe(() => storeUpdates++);
    fireEvent.pointerDown(sep, { clientX: 100, button: 0, buttons: 1, pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 130, buttons: 1, pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 150, buttons: 1, pointerId: 1 });
    expect(ide.style.getPropertyValue("--file-w")).toBe(`${start + 50}px`);
    expect(storeUpdates).toBe(0);
    expect(useStore.getState().layout.sizes.filePanel).toBe(start);
    fireEvent.pointerUp(sep, { clientX: 150, pointerId: 1 });
    unsub();
    expect(useStore.getState().layout.sizes.filePanel).toBe(start + 50);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).layout.sizes.filePanel).toBe(start + 50);
    expect(document.body.classList.contains("resizing")).toBe(false);
  });

  it("ignores non-primary buttons and cancel restores the size", () => {
    const { container } = render(<App />);
    const ide = container.querySelector(".ide") as HTMLElement;
    const sep = screen.getByRole("separator", { name: "Resize file panel" });
    const start = useStore.getState().layout.sizes.filePanel;
    fireEvent.pointerDown(sep, { clientX: 100, button: 2, buttons: 2, pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 200, buttons: 2, pointerId: 1 });
    expect(ide.style.getPropertyValue("--file-w")).toBe(`${start}px`);
    fireEvent.pointerDown(sep, { clientX: 100, button: 0, buttons: 1, pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 160, buttons: 1, pointerId: 1 });
    fireEvent.pointerCancel(sep, { pointerId: 1 });
    expect(ide.style.getPropertyValue("--file-w")).toBe(`${start}px`);
    expect(useStore.getState().layout.sizes.filePanel).toBe(start);
    expect(document.body.classList.contains("resizing")).toBe(false);
  });

  it("resetLayout persists", () => {
    useStore.getState().setSize("agent", 500);
    useStore.getState().resetLayout();
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).layout.sizes.agent).toBe(318);
  });
});

const host: Settings = {
  endpoint: "http://127.0.0.1:11434", model: "m", api_key: "", ignore: ["x"], theme: "harbor", font_size: 13, recent: ["a"],
};

describe("settings ipc", () => {
  it("pickSettings keeps exactly the seven known keys", () => {
    const dirty = { ...host, fontSize: 9, extra: true } as unknown as Settings;
    expect(Object.keys(pickSettings(dirty)).sort()).toEqual(["api_key", "endpoint", "font_size", "ignore", "model", "recent", "theme"]);
  });

  it("setSettings sends only the known keys", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue(host);
    await setSettings({ ...host, junk: 1 } as unknown as Settings);
    const [cmd, args] = invokeMock.mock.calls[0] as [string, { settings: Record<string, unknown> }];
    expect(cmd).toBe("set_settings");
    expect(Object.keys(args.settings).sort()).toEqual(["api_key", "endpoint", "font_size", "ignore", "model", "recent", "theme"]);
  });

  it("setTheme re-fetches settings and merges onto the freshest copy", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    // the store holds a stale copy; the host has a newer model
    useStore.setState({ settings: { value: { ...host, model: "stale" }, loaded: true, error: null } });
    const fresh = { ...host, model: "fresh", font_size: 16 };
    invokeMock.mockImplementation(async (cmd: string, args?: { settings: Settings }) =>
      cmd === "get_settings" ? fresh : args!.settings,
    );
    await useStore.getState().setTheme("ember");
    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(["get_settings", "set_settings"]);
    const sent = (invokeMock.mock.calls[1][1] as { settings: Settings }).settings;
    expect(sent).toMatchObject({ model: "fresh", font_size: 16, theme: "ember" });
  });
});

describe("bootstrapTheme", () => {
  it("applies saved theme and zoom synchronously", () => {
    localStorage.setItem("asa.theme.chosen", "ember");
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ zoom: 1.3 }));
    const root = document.createElement("html");
    expect(bootstrapTheme(root)).toBe("ember");
    expect(root.style.getPropertyValue("--bg")).toBe(themes.ember.bg);
    expect(root.style.getPropertyValue("--zoom")).toBe("1.3");
    expect(root.style.colorScheme).toBe("dark");
  });

  it("falls back to harbor, clamps zoom and survives corrupt storage", () => {
    localStorage.setItem(STORAGE_KEY, "{not json");
    const root = document.createElement("html");
    expect(bootstrapTheme(root)).toBe("harbor");
    expect(root.style.getPropertyValue("--zoom")).toBe("1");
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ zoom: 99 }));
    bootstrapTheme(root);
    expect(root.style.getPropertyValue("--zoom")).toBe("1.6");
  });

  it("uses paper (light color-scheme) when the OS prefers light and nothing is saved", () => {
    const orig = window.matchMedia;
    window.matchMedia = ((q: string) => ({ matches: q.includes("light") })) as unknown as typeof matchMedia;
    const root = document.createElement("html");
    expect(bootstrapTheme(root)).toBe("paper");
    expect(root.style.colorScheme).toBe("light");
    window.matchMedia = orig;
  });
});
