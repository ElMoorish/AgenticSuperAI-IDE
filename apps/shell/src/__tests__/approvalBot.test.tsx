import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ agentPrompt: vi.fn(), agentCancel: vi.fn(), resolveApproval: vi.fn() }));
vi.mock("../ipc", async () => {
  const actual = await vi.importActual<typeof import("../ipc")>("../ipc");
  return { ...actual, ...ipc };
});

import { ApprovalBot, clampPos } from "../agent/ApprovalBot";
import { initialAgentState, reduceAgent, useAgent } from "../agent/agentStore";

afterEach(cleanup);
beforeEach(() => {
  localStorage.clear();
  ipc.resolveApproval.mockReset();
  ipc.resolveApproval.mockResolvedValue(undefined);
  useAgent.setState({ state: initialAgentState(), mode: "build" });
});

const ask = (...evs: Parameters<typeof reduceAgent>[1][]) =>
  useAgent.setState({ state: evs.reduce(reduceAgent, initialAgentState()) });

const edit = (id: number, title = "src/main.py") =>
  ({ type: "approval", id, kind: "edit", title, old: "a\nb\n", new: "a\nc\nd\n" }) as const;
const cmd = (id: number) => ({ type: "approval", id, kind: "command", title: "Run", command: "cargo test" }) as const;

describe("ApprovalBot", () => {
  it("is not shown while nothing waits for a decision", () => {
    render(<ApprovalBot />);
    expect(screen.queryByRole("region", { name: "Approval helper" })).toBeNull();
  });

  it("shows the file and change size of an edit, and Accept sends one yes", () => {
    ask(edit(7));
    render(<ApprovalBot />);
    expect(screen.getByText("src/main.py")).toBeTruthy();
    expect(screen.getByText("+2 −1")).toBeTruthy();
    const accept = screen.getByRole("button", { name: "Accept" });
    fireEvent.click(accept);
    fireEvent.click(accept); // a double click must not decide twice
    expect(ipc.resolveApproval).toHaveBeenCalledTimes(1);
    expect(ipc.resolveApproval).toHaveBeenCalledWith(7, true);
  });

  it("shows a command exactly and Deny refuses it", () => {
    ask(cmd(3));
    render(<ApprovalBot />);
    expect(screen.getByText("cargo test")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(ipc.resolveApproval).toHaveBeenCalledWith(3, false);
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull(); // review is for edits only
  });

  it("takes requests oldest first, one at a time, and says how many wait", () => {
    ask(cmd(1), edit(2));
    render(<ApprovalBot />);
    expect(screen.getByText("+1 waiting")).toBeTruthy();
    expect(screen.getByText("cargo test")).toBeTruthy();
    expect(screen.queryByText("src/main.py")).toBeNull();
    expect(screen.queryByRole("button", { name: /(approve|accept|allow) all/i })).toBeNull(); // no approve-all
  });

  it("moves on to the next request once the first is answered, with fresh buttons", async () => {
    ask(cmd(1), edit(2));
    render(<ApprovalBot />);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect(await screen.findByText("src/main.py")).toBeTruthy(); // the store drops request 1 after the host answers
    const accept = screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement;
    expect(accept.disabled).toBe(false);
  });

  it("minimizes to a bubble with the count and opens again", () => {
    ask(cmd(1), cmd(2));
    render(<ApprovalBot />);
    fireEvent.click(screen.getByRole("button", { name: "Minimize the helper" }));
    const bubble = screen.getByRole("button", { name: /2 requests waiting/ });
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
    fireEvent.click(bubble);
    expect(screen.getByRole("button", { name: "Allow" })).toBeTruthy();
  });

  it("Review opens the side-by-side view for an edit", async () => {
    ask(edit(5));
    render(<ApprovalBot />);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByRole("dialog")).toBeTruthy();
  });
});

describe("clampPos", () => {
  const box = { w: 270, h: 120 };
  const view = { w: 1000, h: 700 };
  it("keeps a dragged helper inside the window", () => {
    expect(clampPos({ x: -50, y: -9 }, box, view)).toEqual({ x: 8, y: 8 });
    expect(clampPos({ x: 5000, y: 5000 }, box, view)).toEqual({ x: 1000 - 270 - 8, y: 700 - 120 - 8 });
    expect(clampPos({ x: 300, y: 200 }, box, view)).toEqual({ x: 300, y: 200 });
  });
  it("copes with a window smaller than the helper", () => {
    expect(clampPos({ x: 100, y: 100 }, box, { w: 200, h: 100 })).toEqual({ x: 8, y: 8 });
  });
});
