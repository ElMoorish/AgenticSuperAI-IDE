import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ agentPrompt: vi.fn(), agentCancel: vi.fn(), resolveApproval: vi.fn() }));
vi.mock("../ipc", async () => {
  const actual = await vi.importActual<typeof import("../ipc")>("../ipc");
  return { ...actual, ...ipc, isTauri: () => true };
});

const bus = vi.hoisted(() => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    handlers,
    emit: vi.fn(async () => undefined),
    listen: vi.fn(async (name: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(name, h);
      return () => handlers.delete(name);
    }),
  };
});
vi.mock("@tauri-apps/api/event", () => ({ emit: bus.emit, listen: bus.listen }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ unminimize: async () => undefined, show: async () => undefined, setFocus: async () => undefined }),
}));

import { BotCard } from "../overlay/BotCard";
import { choosePosition, defaultPosition, isOnScreen, parseSaved } from "../overlay/placement";
import { EV_APPROVALS, EV_DECIDE, EV_READY, parseDecision, toBotItems, type BotItem } from "../overlay/relay";
import { useOverlayRelay } from "../overlay/useOverlayRelay";
import { initialAgentState, reduceAgent, useAgent } from "../agent/agentStore";

afterEach(cleanup);
beforeEach(() => {
  bus.handlers.clear();
  bus.emit.mockClear();
  bus.listen.mockClear();
  ipc.resolveApproval.mockReset();
  ipc.resolveApproval.mockResolvedValue(undefined);
  useAgent.setState({ state: initialAgentState(), mode: "build" });
});

const edit: BotItem = { id: 7, kind: "edit", title: "src/main.py", stat: "+2 −1" };
const cmd: BotItem = { id: 3, kind: "command", title: "Run", command: "cargo test" };

describe("BotCard", () => {
  it("shows the file and change size of an edit; Accept answers once", () => {
    const onDecide = vi.fn();
    render(<BotCard items={[edit]} onDecide={onDecide} onOpen={vi.fn()} />);
    expect(screen.getByText("src/main.py")).toBeTruthy();
    expect(screen.getByText("+2 −1")).toBeTruthy();
    const accept = screen.getByRole("button", { name: "Accept" });
    fireEvent.click(accept);
    fireEvent.click(accept);
    expect(onDecide).toHaveBeenCalledTimes(1);
    expect(onDecide).toHaveBeenCalledWith(7, true);
  });

  it("shows a command exactly; Deny refuses it", () => {
    const onDecide = vi.fn();
    render(<BotCard items={[cmd]} onDecide={onDecide} onOpen={vi.fn()} />);
    expect(screen.getByText("cargo test")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(onDecide).toHaveBeenCalledWith(3, false);
  });

  it("takes the oldest first, counts the rest, and offers no approve-all", () => {
    render(<BotCard items={[cmd, edit]} onDecide={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.getByText("+1 waiting")).toBeTruthy();
    expect(screen.queryByText("src/main.py")).toBeNull();
    expect(screen.queryByRole("button", { name: /(approve|accept|allow) all/i })).toBeNull();
  });

  it("gets fresh buttons when the next request arrives", () => {
    const { rerender } = render(<BotCard items={[cmd]} onDecide={vi.fn()} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect((screen.getByRole("button", { name: "Allow" }) as HTMLButtonElement).disabled).toBe(true);
    rerender(<BotCard items={[edit]} onDecide={vi.fn()} onOpen={vi.fn()} />);
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("Open IDE asks for the IDE window", () => {
    const onOpen = vi.fn();
    render(<BotCard items={[cmd]} onDecide={vi.fn()} onOpen={onOpen} />);
    fireEvent.click(screen.getByRole("button", { name: "Open IDE" }));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });
});

describe("relay messages", () => {
  it("toBotItems sends sizes and names, never file contents", () => {
    const items = toBotItems([
      { id: 1, kind: "edit", title: "a.txt", old: "a\nb\n", new: "a\nc\nd\n" },
      { id: 2, kind: "edit", title: "new.txt", old: "", new: "x\n" },
      { id: 3, kind: "command", title: "Run", command: "ls" },
    ]);
    expect(items[0]).toEqual({ id: 1, kind: "edit", title: "a.txt", stat: "+2 −1", isNew: false });
    expect(items[1].isNew).toBe(true);
    expect(items[2]).toEqual({ id: 3, kind: "command", title: "Run", command: "ls" });
    expect(JSON.stringify(items)).not.toContain("\"old\"");
  });

  it("parseDecision accepts only a numeric id and a boolean answer", () => {
    expect(parseDecision({ id: 4, ok: true })).toEqual({ id: 4, ok: true });
    for (const bad of [null, "x", {}, { id: "4", ok: true }, { id: 4, ok: "yes" }, { id: 1.5, ok: true }]) {
      expect(parseDecision(bad)).toBeNull();
    }
  });
});

describe("useOverlayRelay (IDE window side)", () => {
  const ask = (...evs: Parameters<typeof reduceAgent>[1][]) =>
    act(() => useAgent.setState({ state: evs.reduce(reduceAgent, useAgent.getState().state) }));

  it("sends the waiting list when it changes and when the overlay asks", async () => {
    renderHook(() => useOverlayRelay());
    await waitFor(() => expect(bus.handlers.has(EV_READY)).toBe(true));
    bus.emit.mockClear();
    ask({ type: "approval", id: 5, kind: "command", title: "Run", command: "ls" });
    expect(bus.emit).toHaveBeenCalledWith(EV_APPROVALS, [{ id: 5, kind: "command", title: "Run", command: "ls" }]);
    bus.emit.mockClear();
    bus.handlers.get(EV_READY)!({ payload: null });
    expect(bus.emit).toHaveBeenCalledWith(EV_APPROVALS, expect.any(Array));
  });

  it("carries out a decision for a waiting request, and ignores unknown or malformed ones", async () => {
    renderHook(() => useOverlayRelay());
    await waitFor(() => expect(bus.handlers.has(EV_DECIDE)).toBe(true));
    ask({ type: "approval", id: 5, kind: "command", title: "Run", command: "ls" });
    const decide = bus.handlers.get(EV_DECIDE)!;
    decide({ payload: { id: 99, ok: true } }); // not waiting
    decide({ payload: { id: "5", ok: true } }); // malformed
    expect(ipc.resolveApproval).not.toHaveBeenCalled();
    decide({ payload: { id: 5, ok: false } });
    expect(ipc.resolveApproval).toHaveBeenCalledWith(5, false);
  });
});

describe("placement", () => {
  const work = { x: 0, y: 0, w: 1920, h: 1040 }; // 1080 screen minus a 40 px taskbar
  const size = { w: 340, h: 136 };

  it("starts at the bottom-right of the work area with a margin scaled to the display", () => {
    expect(defaultPosition(work, size, 1)).toEqual({ x: 1920 - 340 - 16, y: 1040 - 136 - 16 });
    expect(defaultPosition(work, { w: 510, h: 204 }, 1.5)).toEqual({ x: 1920 - 510 - 24, y: 1040 - 204 - 24 });
  });

  it("works on a second screen to the left (negative coordinates)", () => {
    const left = { x: -1920, y: 0, w: 1920, h: 1040 };
    expect(defaultPosition(left, size, 1).x).toBe(-340 - 16);
  });

  it("uses the remembered spot only while it is still on a screen", () => {
    const fb = defaultPosition(work, size, 1);
    expect(choosePosition({ x: 300, y: 200 }, [work], fb)).toEqual({ x: 300, y: 200 });
    expect(choosePosition({ x: 5000, y: 200 }, [work], fb)).toEqual(fb); // that monitor was unplugged
    expect(choosePosition(null, [work], fb)).toEqual(fb);
    expect(isOnScreen({ x: 1900, y: 10 }, [work])).toBe(false); // only a sliver left: not grabbable
  });

  it("parses what was stored and rejects junk", () => {
    expect(parseSaved('{"x":10.4,"y":20}')).toEqual({ x: 10, y: 20 });
    for (const bad of [null, "", "nope", '{"x":"a","y":1}', "[]"]) expect(parseSaved(bad)).toBeNull();
  });
});
