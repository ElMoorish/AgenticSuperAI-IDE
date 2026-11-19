import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  agentPrompt: vi.fn(),
  agentCancel: vi.fn(),
  resolveApproval: vi.fn(),
}));
vi.mock("../ipc", async () => {
  const actual = await vi.importActual<typeof import("../ipc")>("../ipc");
  return { ...actual, ...ipc };
});

import { Composer } from "../agent/Composer";
import { Gauges, fmtGpu, fmtTokens } from "../agent/Gauges";
import { initialAgentState, reduceAgent, useAgent } from "../agent/agentStore";
import { IpcError } from "../ipc";

afterEach(cleanup);
beforeEach(() => {
  ipc.agentPrompt.mockReset();
  ipc.agentCancel.mockReset();
  ipc.resolveApproval.mockReset();
  useAgent.setState({ state: initialAgentState(), mode: "build" });
});

describe("Composer", () => {
  const setup = (over: Partial<Parameters<typeof Composer>[0]> = {}) => {
    const props = { mode: "build" as const, onMode: vi.fn(), busy: false, onSend: vi.fn(), onStop: vi.fn(), ...over };
    render(<Composer {...props} />);
    return { props, box: screen.getByLabelText("Message to the agent") as HTMLTextAreaElement };
  };

  it("Enter sends the trimmed text and clears the box; Shift+Enter does not send", () => {
    const { props, box } = setup();
    fireEvent.change(box, { target: { value: "  fix the build  " } });
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(props.onSend).not.toHaveBeenCalled();
    fireEvent.keyDown(box, { key: "Enter" });
    expect(props.onSend).toHaveBeenCalledWith("fix the build");
    expect(box.value).toBe("");
  });

  it("does not send an empty message or while an IME composition is open", () => {
    const { props, box } = setup();
    fireEvent.keyDown(box, { key: "Enter" });
    fireEvent.change(box, { target: { value: "こんにちは" } });
    fireEvent.keyDown(box, { key: "Enter", keyCode: 229 });
    expect(props.onSend).not.toHaveBeenCalled();
  });

  it("shows Stop while the agent is busy and calls onStop; Enter does not send then", () => {
    const { props, box } = setup({ busy: true });
    fireEvent.change(box, { target: { value: "next" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(props.onSend).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Stop the agent" }));
    expect(props.onStop).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
  });

  it("is disabled with an explanation when no folder is open", () => {
    const { box } = setup({ disabledReason: "Open a folder first" });
    expect(box.disabled).toBe(true);
    expect(box.placeholder).toBe("Open a folder first");
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Plan/Build is a radiogroup that reports the choice", () => {
    const { props } = setup();
    expect(screen.getByRole("radio", { name: "Build" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("radio", { name: "Plan" }));
    expect(props.onMode).toHaveBeenCalledWith("plan");
  });
});

describe("Gauges", () => {
  it("formats tokens and GPU memory like the mockup", () => {
    expect(fmtTokens(950)).toBe("950");
    expect(fmtTokens(6200)).toBe("6.2k");
    expect(fmtTokens(16384)).toBe("16k");
    expect(fmtGpu(5939, 8188)).toBe("5.8 / 8 GB");
  });

  it("shows context and GPU, and says so when no GPU is reported", () => {
    let s = reduceAgent(initialAgentState(), { type: "usage", promptTokens: 6000, completionTokens: 200, tokensPerSecond: 30 });
    const { rerender } = render(<Gauges state={s} contextSize={16384} />);
    expect(screen.getByText("6.2k / 16k")).toBeTruthy();
    expect(screen.getByText("not available")).toBeTruthy();
    s = reduceAgent(s, { type: "gpu", usedMiB: 5939, totalMiB: 8188 });
    rerender(<Gauges state={s} contextSize={16384} />);
    expect(screen.getByText("5.8 / 8 GB")).toBeTruthy();
  });
});

describe("useAgent", () => {
  it("send runs a turn through the shared hub listener and ignores a second send while busy", async () => {
    let finish!: () => void;
    ipc.agentPrompt.mockImplementation(() => new Promise<void>((r) => (finish = r)));
    const p = useAgent.getState().send("  hello  ");
    expect(useAgent.getState().state.turn).toBe("running");
    expect(useAgent.getState().state.messages[0]).toMatchObject({ role: "user", text: "hello" });
    await useAgent.getState().send("second"); // busy: dropped
    expect(ipc.agentPrompt).toHaveBeenCalledTimes(1);
    expect(ipc.agentPrompt.mock.calls[0][0]).toBe("build");
    expect(ipc.agentPrompt.mock.calls[0][1]).toBe("hello");
    finish();
    await p;
    expect(useAgent.getState().state.turn).toBe("idle");
  });

  it("a rejected prompt becomes a readable banner and the panel is usable again", async () => {
    ipc.agentPrompt.mockRejectedValue(new IpcError("agent_missing", "agent-core executable not found"));
    await useAgent.getState().send("hi");
    const s = useAgent.getState().state;
    expect(s.turn).toBe("idle");
    expect(s.banner).toMatch(/agent-core executable not found/);
  });

  it("resolve: an unknown (stale/replayed) approval id is dropped silently; a real failure is shown", async () => {
    const a = { type: "approval" as const, id: 5, kind: "command" as const, title: "Run", command: "cl main.cpp" };
    useAgent.getState().dispatch(a);
    ipc.resolveApproval.mockRejectedValueOnce(new IpcError("unknown_approval", "gone"));
    await useAgent.getState().resolve(5, true);
    expect(useAgent.getState().state.approvals).toEqual([]);
    expect(useAgent.getState().state.banner).toBeUndefined();

    useAgent.getState().dispatch({ ...a, id: 6 });
    ipc.resolveApproval.mockRejectedValueOnce(new IpcError("conflict", "file changed on disk"));
    await useAgent.getState().resolve(6, true);
    expect(useAgent.getState().state.approvals).toEqual([]);
    expect(useAgent.getState().state.banner).toMatch(/file changed on disk/);
  });

  it("cancel never throws even when nothing is running", async () => {
    ipc.agentCancel.mockRejectedValue(new IpcError("io", "nothing"));
    await expect(useAgent.getState().cancel()).resolves.toBeUndefined();
  });
});
