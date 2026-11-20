import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAgent } from "../agent/agentStore";
import { StatusBar } from "../layout/StatusBar";
import { TitleBar } from "../layout/TitleBar";
import { SettingsDialog } from "../settings/SettingsDialog";
import { useModelStatus, validateEndpoint } from "../settings/modelStatus";
import { useStore } from "../state/store";

const probe = vi.fn();
const setSettingsMock = vi.fn(async (s: unknown) => s);
vi.mock("../ipc", async (orig) => ({
  ...(await orig<typeof import("../ipc")>()),
  probeEndpoint: (u: string) => probe(u),
  detectServers: async () => [{ name: "Ollama", url: "http://localhost:11434" }],
  getSettings: async () => useStore.getState().settings.value,
  setSettings: (s: unknown) => setSettingsMock(s),
  windowControl: async () => {},
}));

const base = { endpoint: "http://localhost:11434", model: "qwen", api_key: "", ignore: ["node_modules"], theme: "voltage", font_size: 13, recent: [] };

beforeEach(() => {
  localStorage.clear();
  probe.mockReset();
  setSettingsMock.mockClear();
  useStore.setState({ settings: { value: base, loaded: true, error: null } });
  useModelStatus.setState({ state: "unknown", models: [], error: null, endpoint: "" });
  useAgent.setState({ state: { ...useAgent.getState().state, gpu: undefined, usage: { prompt: 0, completion: 0, tps: 0 } } });
});
afterEach(cleanup);

describe("model chip", () => {
  it("model_chip_shows_down_state_when_probe_fails", async () => {
    probe.mockResolvedValue({ ok: false, models: [], error: "connection refused" });
    render(<TitleBar />);
    const chip = await screen.findByRole("button", { name: /server not reachable/i });
    expect(chip.textContent).toMatch(/offline/);
    expect(chip.className).toContain("chip-down");
  });

  it("shows connected when the probe succeeds", async () => {
    probe.mockResolvedValue({ ok: true, models: ["qwen"], error: null });
    render(<TitleBar />);
    expect(await screen.findByRole("button", { name: /connected/i })).toBeTruthy();
  });
});

describe("settings dialog", () => {
  it("settings_dialog_validates_url", async () => {
    probe.mockResolvedValue({ ok: true, models: ["a", "b"], error: null });
    render(<SettingsDialog onClose={() => {}} />);
    const input = screen.getByLabelText("Model server address");
    fireEvent.change(input, { target: { value: "localhost:11434" } });
    expect(screen.getByRole("alert").textContent).toMatch(/http/);
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(input, { target: { value: "ftp://x" } });
    expect(screen.getByRole("alert").textContent).toMatch(/http:\/\/ or https:\/\//);
    fireEvent.change(input, { target: { value: "http://127.0.0.1:1234" } });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false);
    expect(validateEndpoint("")).toMatch(/Enter/);
  });

  it("saves merged settings, applies context size and closes", async () => {
    probe.mockResolvedValue({ ok: true, models: ["qwen", "llama"], error: null });
    const onClose = vi.fn();
    render(<SettingsDialog onClose={onClose} />);
    await waitFor(() => expect(screen.getByRole("status").textContent).toMatch(/Connected/));
    fireEvent.change(screen.getByLabelText("Editor font size"), { target: { value: "15" } });
    fireEvent.change(screen.getByLabelText("Context size (tokens)"), { target: { value: "32768" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(setSettingsMock).toHaveBeenCalledWith(expect.objectContaining({ font_size: 15, endpoint: "http://localhost:11434", ignore: ["node_modules"] }));
    expect(useAgent.getState().contextSize).toBe(32768);
  });

  it("rejects an out-of-range font size", () => {
    probe.mockResolvedValue({ ok: true, models: [], error: null });
    render(<SettingsDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("Editor font size"), { target: { value: "2" } });
    expect(screen.getByText(/Font size must be/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("status bar", () => {
  it("status_shows_tps_and_gpu", () => {
    render(<StatusBar />);
    expect(screen.getByText(/not available/)).toBeTruthy();
    act(() => {
      useAgent.getState().dispatch({ type: "usage", promptTokens: 100, completionTokens: 20, tokensPerSecond: 42.5 });
      useAgent.getState().dispatch({ type: "gpu", usedMiB: 6144, totalMiB: 8188 });
    });
    expect(screen.getByText("42.5 tok/s")).toBeTruthy();
    expect(screen.getByText("6.0 / 8 GB")).toBeTruthy();
  });
});

describe("emergency stop", () => {
  it("Ctrl+Alt+Esc cancels the agent turn, even from inside an editor", async () => {
    const { handleShortcut } = await import("../App");
    const cancel = vi.fn(async () => {});
    useAgent.setState({ cancel });
    const host = document.createElement("div");
    host.className = "monaco-editor";
    document.body.appendChild(host);
    const ev = new KeyboardEvent("keydown", { key: "Escape", ctrlKey: true, altKey: true, bubbles: true, cancelable: true });
    host.dispatchEvent(ev);
    expect(handleShortcut(ev)).toBe(true);
    expect(cancel).toHaveBeenCalledTimes(1);
    host.remove();
  });
});
