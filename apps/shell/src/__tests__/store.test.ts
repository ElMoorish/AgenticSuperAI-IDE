import { beforeEach, describe, expect, it } from "vitest";
import { LIMITS, ZOOM_MAX, ZOOM_MIN, clampSize, clampZoom, useStore } from "../state/store";

beforeEach(() => {
  localStorage.clear();
  useStore.getState().resetLayout();
});

describe("store", () => {
  it("layout clamp respects min/max sizes", () => {
    expect(clampSize("filePanel", 1)).toBe(LIMITS.filePanel.min);
    expect(clampSize("filePanel", 99999)).toBe(LIMITS.filePanel.max);
    expect(clampSize("agent", 400)).toBe(400);
    expect(clampSize("bottom", Number.NaN)).toBe(LIMITS.bottom.min);
    useStore.getState().setSize("agent", 5000);
    expect(useStore.getState().layout.sizes.agent).toBe(LIMITS.agent.max);
    useStore.getState().setSize("bottom", -10);
    expect(useStore.getState().layout.sizes.bottom).toBe(LIMITS.bottom.min);
  });

  it("zoom clamp", () => {
    expect(clampZoom(0.1)).toBe(ZOOM_MIN);
    expect(clampZoom(9)).toBe(ZOOM_MAX);
    expect(clampZoom(Number.NaN)).toBe(1);
    expect(ZOOM_MIN).toBe(0.8);
    expect(ZOOM_MAX).toBe(1.6);
    const s = useStore.getState();
    s.zoomIn(); s.zoomIn();
    expect(useStore.getState().zoom).toBeCloseTo(1.2);
    for (let i = 0; i < 20; i++) useStore.getState().zoomIn();
    expect(useStore.getState().zoom).toBe(ZOOM_MAX);
    for (let i = 0; i < 30; i++) useStore.getState().zoomOut();
    expect(useStore.getState().zoom).toBe(ZOOM_MIN);
    useStore.getState().zoomReset();
    expect(useStore.getState().zoom).toBe(1);
  });

  it("collapsed state toggles and persists layout", () => {
    useStore.getState().toggleCollapsed("agent");
    expect(useStore.getState().layout.collapsed.agent).toBe(true);
    useStore.getState().setSize("filePanel", 250);
    const raw = JSON.parse(localStorage.getItem("asa.ui.v1")!);
    expect(raw.layout.collapsed.agent).toBe(true);
    expect(raw.layout.sizes.filePanel).toBe(250);
  });
});
