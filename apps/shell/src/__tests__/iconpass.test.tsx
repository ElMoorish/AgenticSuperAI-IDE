import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SpeedGauge, pushSpeed, sparkPoints } from "../agent/Gauges";
import { StepList } from "../agent/StepList";
import { terminalTheme } from "../terminal/terminalTheme";
import { AgentCore } from "../ui/AgentCore";
import { Brand } from "../ui/Brand";
import { useStore } from "../state/store";
import { applyTheme, themes } from "../styles/themes";

afterEach(cleanup);

describe("speed history", () => {
  it("keeps the last 60 readings and ignores zero, negative and non-numbers", () => {
    let h: number[] = [];
    for (let i = 1; i <= 70; i++) h = pushSpeed(h, i);
    expect(h).toHaveLength(60);
    expect(h[0]).toBe(11);
    expect(h[59]).toBe(70);
    expect(pushSpeed(h, 0)).toBe(h);
    expect(pushSpeed(h, -3)).toBe(h);
    expect(pushSpeed(h, Number.NaN)).toBe(h);
  });

  it("sparkPoints stays inside the box and rises with speed", () => {
    expect(sparkPoints([], 120, 28)).toBe("");
    const pts = sparkPoints([1, 5, 10], 120, 28).split(" ").map((p) => p.split(",").map(Number));
    expect(pts[0][0]).toBe(0);
    expect(pts[2][0]).toBe(120);
    for (const [, y] of pts) {
      expect(y).toBeGreaterThanOrEqual(0);
      expect(y).toBeLessThanOrEqual(28);
    }
    expect(pts[2][1]).toBeLessThan(pts[0][1]); // faster = higher on screen = smaller y
  });

  it("always shows the number as text, with a dash when nothing is generating", () => {
    const { rerender } = render(<SpeedGauge tps={0} />);
    expect(screen.getByText("-")).toBeTruthy();
    rerender(<SpeedGauge tps={6.44} />);
    expect(screen.getByText("6.4 tok/s")).toBeTruthy();
    expect(screen.getByRole("img").getAttribute("aria-label")).toContain("6.4 tok/s");
  });
});

describe("icons and brand", () => {
  it("step status keeps its screen-reader text and shows an icon, not a glyph", () => {
    const { container } = render(
      <StepList steps={[{ id: 1, seq: 1, title: "Read a.txt", status: "done" }, { id: 2, seq: 2, title: "Run tests", status: "run" }] as never} />,
    );
    expect(screen.getByText("(done)")).toBeTruthy();
    expect(screen.getByText("(running)")).toBeTruthy();
    expect(container.querySelectorAll("li i svg")).toHaveLength(2);
    expect(container.textContent).not.toMatch(/[✓●✕○]/);
  });

  it("the brand mark is decorative and the name stays readable", () => {
    const { container } = render(<Brand />);
    expect(screen.getByText("ASA")).toBeTruthy();
    expect(container.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("the agent core follows the theme and only pulses while busy", () => {
    useStore.setState({ theme: "forge" });
    const { container, rerender } = render(<AgentCore busy={false} />);
    expect(container.querySelector(".core")?.className).toContain("core-forge");
    expect(container.querySelector(".core")?.className).toContain("idle");
    rerender(<AgentCore busy />);
    expect(container.querySelector(".core")?.className).toContain("busy");
    useStore.setState({ theme: "voltage" });
  });
});

describe("terminal colours", () => {
  it("come from the live theme variables, so a theme change changes the terminal", () => {
    applyTheme("voltage");
    expect(terminalTheme().background).toBe(themes.voltage.bg);
    expect(terminalTheme().cursor).toBe(themes.voltage.accent);
    applyTheme("drafting");
    expect(terminalTheme().background).toBe(themes.drafting.bg);
    expect(terminalTheme().foreground).toBe(themes.drafting.text);
    applyTheme("voltage");
  });
});

import { EngineStrip, cellsForBurst, pipeSeconds } from "../ui/EngineStrip";
import { voiceLine } from "../agent/AgentPanel";
import { initialAgentState, reduceAgent } from "../agent/agentStore";

describe("engine strip", () => {
  it("burst size is bounded and zero for nothing", () => {
    expect(cellsForBurst(0)).toBe(0);
    expect(cellsForBurst(-5)).toBe(0);
    expect(cellsForBurst(1)).toBe(1);
    expect(cellsForBurst(10000)).toBe(24);
  });
  it("pipes stand still without disk traffic and speed up with it", () => {
    expect(pipeSeconds(undefined)).toBeNull();
    expect(pipeSeconds(0.5)).toBeNull();
    expect(pipeSeconds(2000)!).toBeLessThan(pipeSeconds(10)!);
  });
  it("renders honest text when nothing is measured yet", () => {
    render(<EngineStrip />);
    expect(screen.getByText("Token stream")).toBeTruthy();
    expect(screen.getAllByText("no data").length).toBeGreaterThan(0);
  });
  it("voice line says what is happening", () => {
    expect(voiceLine({ turn: "idle", steps: [], approvals: [] })).toBe("Ready");
    expect(voiceLine({ turn: "running", steps: [], approvals: [] })).toBe("Thinking");
    expect(voiceLine({ turn: "running", steps: [{ id: 1, seq: 1, title: "Read a.txt", status: "run" }] as never, approvals: [] })).toBe("Read a.txt");
    expect(voiceLine({ turn: "waiting", steps: [], approvals: [] })).toBe("Waiting for your OK");
  });
  it("sys events reach the state", () => {
    const n = reduceAgent(initialAgentState(), { type: "sys", ramUsedMiB: 100, ramTotalMiB: 200, diskReadMBps: 5 } as never);
    expect(n.sys).toEqual({ ramUsedMiB: 100, ramTotalMiB: 200, diskReadMBps: 5 });
  });
});
