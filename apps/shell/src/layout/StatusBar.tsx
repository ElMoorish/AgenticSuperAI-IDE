import { contextBreakdown, useAgent } from "../agent/agentStore";
import { fmtGpu } from "../agent/Gauges";
import { useStore } from "../state/store";

/** Placeholder fields: later tasks feed model, GPU and context numbers into the store. */
export function StatusBar() {
  const zoom = useStore((s) => s.zoom);
  const zoomIn = useStore((s) => s.zoomIn);
  const zoomOut = useStore((s) => s.zoomOut);
  const zoomReset = useStore((s) => s.zoomReset);
  const model = useStore((s) => s.settings.value?.model);
  const agent = useAgent((s) => s.state);
  const contextSize = useAgent((s) => s.contextSize);
  const ctx = contextBreakdown(agent, contextSize);
  const folder = useStore((s) => s.workspace.info?.name);

  return (
    <footer className="status" role="contentinfo">
      <span>Model <b>{model || "none"}</b></span>
      <span>GPU <b className="mono">{agent.gpu ? fmtGpu(agent.gpu.usedMiB, agent.gpu.totalMiB) : "not available"}</b></span>
      <span>Speed <b className="mono">{agent.usage.tps > 0 ? `${agent.usage.tps.toFixed(1)} tok/s` : "-"}</b></span>
      <span>Context <b className="mono">{ctx.used.toLocaleString("en-US")} / {ctx.total.toLocaleString("en-US")}</b></span>
      <span>{folder ?? "No folder"}</span>
      <span className="r">
        <button type="button" aria-label="Zoom out" onClick={zoomOut}>-</button>
        <button type="button" aria-label="Reset zoom" onClick={zoomReset}>{Math.round(zoom * 100)}%</button>
        <button type="button" aria-label="Zoom in" onClick={zoomIn}>+</button>
        <span>Stop agent: <b className="mono">Ctrl+Alt+Esc</b></span>
      </span>
    </footer>
  );
}
