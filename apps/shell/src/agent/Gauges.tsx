import { useEffect, useState } from "react";
import { contextBreakdown, type AgentState } from "./agentStore";

export const fmtTokens = (n: number): string => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(Math.round(n)));
export const fmtGpu = (usedMiB: number, totalMiB: number): string => `${(usedMiB / 1024).toFixed(1)} / ${Math.round(totalMiB / 1024)} GB`;

/** Context budget (system / chat / tool segments) and GPU memory, as in the approved mockup. */
export function Gauges({ state, contextSize }: { state: AgentState; contextSize: number }) {
  const c = contextBreakdown(state, contextSize);
  const seg = (n: number) => (c.total > 0 ? Math.min(100, (n / c.total) * 100) : 0);
  const ctxLabel = `${fmtTokens(c.used)} / ${fmtTokens(c.total)}`;
  const warnCtx = c.fraction >= 0.9;
  const gpu = state.gpu;
  const gpuFrac = gpu && gpu.totalMiB > 0 ? Math.min(1, gpu.usedMiB / gpu.totalMiB) : 0;
  return (
    <div className="gauges">
      <div className="gauge">
        <div className="lab">
          <span>Context</span>
          <b className="mono">{ctxLabel}</b>
        </div>
        <div
          className={`bar${warnCtx ? " hot" : ""}`}
          role="img"
          aria-label={`Context: ${ctxLabel} tokens used (system ${fmtTokens(c.system)}, chat ${fmtTokens(c.chat)}, tools ${fmtTokens(c.tool)})`}
        >
          <i className="seg sys" style={{ width: `${seg(c.system)}%` }} />
          <i className="seg chat" style={{ width: `${seg(c.chat)}%` }} />
          <i className="seg tool" style={{ width: `${seg(c.tool)}%` }} />
        </div>
      </div>
      <div className="gauge">
        <div className="lab">
          <span>GPU memory</span>
          <b className="mono">{gpu ? fmtGpu(gpu.usedMiB, gpu.totalMiB) : "not available"}</b>
        </div>
        <div
          className={`bar${gpuFrac >= 0.9 ? " hot" : ""}`}
          role="img"
          aria-label={gpu ? `GPU memory: ${fmtGpu(gpu.usedMiB, gpu.totalMiB)}` : "GPU memory not available"}
        >
          <i className="seg gpu" style={{ width: `${gpuFrac * 100}%` }} />
        </div>
      </div>
    </div>
  );
}

const HISTORY = 60;

/** The last minute of generation speeds, newest on the right. Pure so it can be tested. */
export function pushSpeed(history: number[], tps: number): number[] {
  if (!(tps > 0) || !Number.isFinite(tps)) return history;
  return [...history, tps].slice(-HISTORY);
}

/** Polyline points for an SVG viewBox of `w` x `h`; the top of the box is `max` tok/s (at least 1). */
export function sparkPoints(history: number[], w: number, h: number): string {
  if (history.length === 0) return "";
  const max = Math.max(1, ...history) * 1.15;
  const step = history.length > 1 ? w / (history.length - 1) : 0;
  return history.map((v, i) => `${(i * step).toFixed(1)},${(h - 2 - (v / max) * (h - 4)).toFixed(1)}`).join(" ");
}

/** Generation speed: the number is always shown as text; the line is a bonus. */
export function SpeedGauge({ tps }: { tps: number }) {
  const [hist, setHist] = useState<number[]>([]);
  useEffect(() => {
    setHist((h) => pushSpeed(h, tps));
  }, [tps]);
  const label = tps > 0 ? `${tps.toFixed(1)} tok/s` : "-";
  const pts = sparkPoints(hist, 120, 28);
  return (
    <div className="gauge speed">
      <div className="lab">
        <span>Speed</span>
        <b className="mono">{label}</b>
      </div>
      <svg className="spark" viewBox="0 0 120 28" preserveAspectRatio="none" role="img" aria-label={`Speed over the last ${hist.length} readings, now ${label}`}>
        {pts && <polyline points={`0,28 ${pts} 120,28`} className="area" />}
        {pts && <polyline points={pts} className="line" fill="none" />}
      </svg>
    </div>
  );
}
