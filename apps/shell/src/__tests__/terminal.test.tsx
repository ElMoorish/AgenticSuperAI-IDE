import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentTerminal } from "../terminal/AgentTerminal";
import { useAgentTerminals } from "../terminal/agentTerminals";
import { stripAnsi, TerminalBuffer } from "../terminal/terminalBuffer";

vi.mock("../ipc", async (orig) => ({ ...(await orig<typeof import("../ipc")>()), terminalWrite: vi.fn() }));

afterEach(() => { cleanup(); useAgentTerminals.getState().clear(); });

describe("terminal buffer", () => {
  it("terminal_buffer_drops_oldest_beyond_cap", () => {
    const b = new TerminalBuffer(10);
    b.append("12345");
    b.append("67890");
    expect(b.text()).toBe("1234567890");
    expect(b.dropped).toBe(false);
    b.append("ABC");
    expect(b.text()).toBe("4567890ABC");
    expect(b.length).toBe(10);
    expect(b.dropped).toBe(true);
    b.append("x".repeat(50)); // a single chunk bigger than the cap keeps only its tail
    expect(b.text()).toBe("x".repeat(10));
  });

  it("strips escape sequences and normalises line endings", () => {
    expect(stripAnsi("\x1b[31mred\x1b[0m\r\nok\x1b]0;title\x07!")).toBe("red\nok!");
  });
});

describe("agent terminals", () => {
  it("agent_terminal_is_read_only", async () => {
    const ipc = await import("../ipc");
    const st = useAgentTerminals.getState();
    st.apply({ type: "agentTerminalOutput", id: 3, chunk: "\x1b[32mhello\x1b[0m\n" });
    st.apply({ type: "agentTerminalExit", id: 3, code: 1 });
    const term = useAgentTerminals.getState().terms[0];
    const { container } = render(<AgentTerminal term={term} />);
    expect(screen.getByRole("log").textContent).toContain("hello");
    expect(screen.getByRole("log").textContent).not.toContain("\x1b");
    expect(screen.getByText(/Exited with code 1/)).toBeTruthy();
    expect(container.querySelector("input, textarea, [contenteditable]")).toBeNull();
    expect(ipc.terminalWrite).not.toHaveBeenCalled();
  });

  it("keeps terminals separate and bounded in number", () => {
    const st = useAgentTerminals.getState();
    for (let i = 1; i <= 15; i++) st.apply({ type: "agentTerminalOutput", id: i, chunk: "x" });
    const ids = useAgentTerminals.getState().terms.map((t) => t.id);
    expect(ids).toHaveLength(12);
    expect(ids[0]).toBe(4);
  });
});
