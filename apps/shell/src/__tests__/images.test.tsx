import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ agentPrompt: vi.fn(), agentCancel: vi.fn(), resolveApproval: vi.fn() }));
vi.mock("../ipc", async () => {
  const actual = await vi.importActual<typeof import("../ipc")>("../ipc");
  return { ...actual, ...ipc };
});

import { Composer } from "../agent/Composer";
import { IMAGE_ONLY_PROMPT, initialAgentState, reduceAgent, useAgent } from "../agent/agentStore";
import { MAX_IMAGES, prepareImage } from "../agent/images";

afterEach(cleanup);
beforeEach(() => {
  ipc.agentPrompt.mockReset();
  useAgent.setState({ state: initialAgentState(), mode: "build" });
});

const png = (name = "shot.png") => new File([new Uint8Array([137, 80, 78, 71])], name, { type: "image/png" });

describe("prepareImage", () => {
  it("gives base64 without the data: prefix plus a thumbnail URL", async () => {
    const a = await prepareImage(png(), "shot.png");
    expect(a.mime).toBe("image/png");
    expect(a.data).toBe("iVBORw==");
    expect(a.url).toBe("data:image/png;base64,iVBORw==");
  });

  it("refuses other file types and oversized pictures", async () => {
    await expect(prepareImage(new File(["x"], "a.html", { type: "text/html" }))).rejects.toThrow(/PNG, JPEG/);
    const big = new File([new Uint8Array(5_000_001)], "big.png", { type: "image/png" });
    await expect(prepareImage(big)).rejects.toThrow(/5 MB/);
  });
});

describe("Composer pictures", () => {
  const setup = () => {
    const props = { mode: "build" as const, onMode: vi.fn(), busy: false, onSend: vi.fn(), onStop: vi.fn() };
    render(<Composer {...props} />);
    return { props, box: screen.getByLabelText("Message to the agent") as HTMLTextAreaElement };
  };

  it("a pasted picture becomes a thumbnail and is sent with the text", async () => {
    const { props, box } = setup();
    fireEvent.paste(box, { clipboardData: { files: [png()] } });
    await screen.findByAltText("shot.png");
    fireEvent.change(box, { target: { value: "why is this red?" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(props.onSend).toHaveBeenCalledTimes(1);
    const [text, images] = props.onSend.mock.calls[0];
    expect(text).toBe("why is this red?");
    expect(images).toHaveLength(1);
    expect(images[0].mime).toBe("image/png");
    expect(screen.queryByAltText("shot.png")).toBeNull(); // cleared after sending
  });

  it("a picture alone can be sent, and the thumbnail can be removed first", async () => {
    const { props, box } = setup();
    fireEvent.paste(box, { clipboardData: { files: [png("a.png"), png("b.png")] } });
    await screen.findByAltText("b.png");
    fireEvent.click(screen.getByRole("button", { name: "Remove a.png" }));
    expect(screen.queryByAltText("a.png")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(props.onSend.mock.calls[0][0]).toBe("");
    expect(props.onSend.mock.calls[0][1]).toHaveLength(1);
  });

  it("a dropped picture is attached; a dropped non-picture is ignored", async () => {
    setup();
    const zone = document.querySelector(".composer") as HTMLElement;
    fireEvent.drop(zone, { dataTransfer: { files: [new File(["x"], "n.txt", { type: "text/plain" })], types: ["Files"] } });
    expect(screen.queryByRole("list", { name: "Attached pictures" })).toBeNull();
    fireEvent.drop(zone, { dataTransfer: { files: [png("d.png")], types: ["Files"] } });
    await screen.findByAltText("d.png");
  });

  it("stops at the picture limit and says so", async () => {
    const { box } = setup();
    fireEvent.paste(box, { clipboardData: { files: Array.from({ length: MAX_IMAGES + 1 }, (_, i) => png(`p${i}.png`)) } });
    await screen.findByAltText(`p${MAX_IMAGES - 1}.png`);
    expect(screen.queryByAltText(`p${MAX_IMAGES}.png`)).toBeNull();
    expect((await screen.findByRole("alert")).textContent).toMatch(/At most 4/);
  });
});

describe("pictures in the agent store", () => {
  it("promptSent keeps the thumbnails on the user message", () => {
    const s = reduceAgent(initialAgentState(), { type: "promptSent", text: "look", images: ["data:image/png;base64,AA=="] });
    expect(s.messages[0].images).toEqual(["data:image/png;base64,AA=="]);
    expect(reduceAgent(initialAgentState(), { type: "promptSent", text: "x" }).messages[0].images).toBeUndefined();
  });

  it("send passes the images to the host and asks a default question when only a picture is sent", async () => {
    ipc.agentPrompt.mockResolvedValue(undefined);
    await useAgent.getState().send("", [{ id: 1, mime: "image/png", data: "AA==", url: "data:image/png;base64,AA==", name: "x" }]);
    const [mode, text, , images] = ipc.agentPrompt.mock.calls[0];
    expect(mode).toBe("build");
    expect(text).toBe(IMAGE_ONLY_PROMPT);
    expect(images).toEqual([{ mime: "image/png", data: "AA==" }]);
    await waitFor(() => expect(useAgent.getState().state.turn).toBe("idle"));
  });
});
