import { useEffect, useRef } from "react";
import { Brain, Cpu, HardDrive, Layers, MemoryStick } from "lucide-react";
import { SpeedGauge, fmtGpu } from "../agent/Gauges";
import { useAgent } from "../agent/agentStore";
import { useStore } from "../state/store";
import type { ThemeName } from "../styles/themes";

const COLS = 32;
const ROWS = 8;
const CELLS = COLS * ROWS;

/** Icon path data (24x24, Lucide) drawn on the canvas, one symbol per theme. */
const PATHS: Record<ThemeName, string> = {
  voltage: "M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z",
  forge: "M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z",
  aurora: "M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z",
  drafting: "M5 12h14 M12 5v14",
  deepsea: "M12 22a7 7 0 0 0 7-7c0-2-1-3.9-3-5.5s-3.5-4-4-6.5c-.5 2.5-2 4.9-4 6.5C6 11.1 5 13 5 15a7 7 0 0 0 7 7z",
  monolith: "M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2",
};

const rgb = (hex: string): [number, number, number] => {
  const h = hex.trim().replace("#", "");
  return h.length === 6 ? [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number] : [128, 128, 128];
};

/** How many cells a burst of streamed text lights: a few per burst, never more than 24. */
export function cellsForBurst(chars: number): number {
  if (!(chars > 0)) return 0;
  return Math.min(24, Math.max(1, Math.ceil(chars / 4)));
}

/** Seconds per loop of a flowing pipe: faster for more MB/s; a still pipe when nothing moves. */
export function pipeSeconds(mbps: number | undefined): number | null {
  if (!mbps || !(mbps > 1)) return null;
  return Math.max(0.35, 2.4 - Math.log10(mbps) * 0.7);
}

/**
 * Cells light up as text streams in from the model. This is a view of the real stream (each burst of text
 * lights some cells), not of which experts the model used: llama.cpp does not report that.
 */
function StreamMap() {
  const theme = useStore((s) => s.theme);
  const cv = useRef<HTMLCanvasElement>(null);
  const heat = useRef(new Float32Array(CELLS));
  const raf = useRef(0);
  const lastLen = useRef(0);
  const streamLen = useAgent((s) => {
    const m = s.state.messages[s.state.messages.length - 1];
    return s.state.streaming && m ? m.text.length : 0;
  });

  const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const draw = () => {
    const c = cv.current;
    if (!c) return false;
    const d = window.devicePixelRatio || 1;
    const w = c.clientWidth, h = c.clientHeight;
    if (!w || !h) return false;
    if (c.width !== Math.round(w * d) || c.height !== Math.round(h * d)) {
      c.width = Math.round(w * d);
      c.height = Math.round(h * d);
    }
    const x = c.getContext("2d");
    if (!x) return false;
    x.setTransform(d, 0, 0, d, 0, 0);
    x.clearRect(0, 0, w, h);
    const css = getComputedStyle(document.documentElement);
    const hot = rgb(css.getPropertyValue("--accent")), cool = rgb(css.getPropertyValue("--accent-2"));
    const off = css.getPropertyValue("--line").trim() || "#333";
    const cw = w / COLS, ch = h / ROWS, size = Math.min(cw, ch) * 0.9;
    const path = new Path2D(PATHS[theme]);
    let alive = false;
    for (let i = 0; i < CELLS; i++) {
      const v = heat.current[i];
      const cx = (i % COLS) * cw + cw / 2, cy = Math.floor(i / COLS) * ch + ch / 2;
      const on = v > 0.03;
      const col = on ? `rgba(${cool.map((q, k) => Math.round(q + (hot[k] - q) * v)).join(",")},${0.35 + 0.65 * v})` : off;
      x.save();
      x.translate(cx - size / 2, cy - size / 2);
      x.scale(size / 24, size / 24);
      x.lineWidth = 2.2; x.lineJoin = "round"; x.lineCap = "round";
      x.strokeStyle = col; x.fillStyle = col;
      if (on && v > 0.45) { x.shadowColor = col; x.shadowBlur = (12 * 24) / size; }
      x.stroke(path);
      if (on && v > 0.2) { x.globalAlpha = 0.5 * v; x.fill(path); }
      x.restore();
      if (on) { alive = true; if (!reduce) heat.current[i] = Math.max(0, v - 0.025); }
    }
    return alive;
  };

  const loop = () => {
    raf.current = draw() ? requestAnimationFrame(loop) : 0;
  };

  useEffect(() => {
    const delta = streamLen - lastLen.current;
    lastLen.current = streamLen;
    const n = cellsForBurst(delta);
    if (n === 0) return;
    for (let k = 0; k < n; k++) heat.current[Math.floor(Math.random() * CELLS)] = 1;
    if (reduce) {
      draw();
      const t = window.setTimeout(() => { heat.current.fill(0); draw(); }, 500);
      return () => window.clearTimeout(t);
    }
    if (!raf.current) raf.current = requestAnimationFrame(loop);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streamLen]);

  useEffect(() => {
    draw(); // repaint on theme change and first mount
    return () => { if (raf.current) cancelAnimationFrame(raf.current); raf.current = 0; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme]);

  return <canvas ref={cv} className="smap" role="img" aria-label="Activity map: cells light up as the model's text streams in" />;
}

function Tier({ icon, name, value, frac }: { icon: React.ReactNode; name: string; value: string; frac: number }) {
  return (
    <div className="tier">
      <span className="tier-ico" aria-hidden="true">{icon}</span>
      <small>{name}</small>
      <b>{value}</b>
      <div className="tbar" role="img" aria-label={`${name}: ${value}`}><u style={{ width: `${Math.round(Math.min(1, Math.max(0, frac)) * 100)}%` }} /></div>
    </div>
  );
}

function Pipe({ seconds }: { seconds: number | null }) {
  return <span className={`pipe${seconds === null ? " still" : ""}`} style={seconds === null ? undefined : { animationDuration: `${seconds}s` }} aria-hidden="true" />;
}

/** The "engine room": what the model is doing, how fast, and where it lives (disk, memory, GPU). */
export function EngineStrip() {
  const agent = useAgent((s) => s.state);
  const { gpu, sys, usage } = agent;
  const disk = sys ? `${sys.diskReadMBps >= 10 ? Math.round(sys.diskReadMBps) : sys.diskReadMBps.toFixed(1)} MB/s` : "no data";
  const ram = sys ? `${(sys.ramUsedMiB / 1024).toFixed(1)} / ${Math.round(sys.ramTotalMiB / 1024)} GB` : "no data";
  const gpuTxt = gpu ? fmtGpu(gpu.usedMiB, gpu.totalMiB) : "no data";
  return (
    <section className="engine" aria-label="Engine">
      <div className="epane">
        <h6><span className="l"><Brain size={12} aria-hidden="true" />Token stream</span><b>{usage.completion > 0 ? `${usage.completion.toLocaleString("en-US")} tokens` : "idle"}</b></h6>
        <StreamMap />
      </div>
      <div className="epane">
        <SpeedGauge tps={usage.tps} />
      </div>
      <div className="epane">
        <h6><span className="l"><Layers size={12} aria-hidden="true" />Where the model lives</span><b>{disk}</b></h6>
        <div className="tiers">
          <Tier icon={<HardDrive size={18} />} name="Disk read" value={disk} frac={sys ? Math.min(1, sys.diskReadMBps / 3500) : 0} />
          <Pipe seconds={pipeSeconds(sys?.diskReadMBps)} />
          <Tier icon={<MemoryStick size={18} />} name="RAM" value={ram} frac={sys && sys.ramTotalMiB > 0 ? sys.ramUsedMiB / sys.ramTotalMiB : 0} />
          <Pipe seconds={usage.tps > 0 ? Math.max(0.35, 2.4 - Math.log10(usage.tps * 50) * 0.7) : null} />
          <Tier icon={<Cpu size={18} />} name="GPU" value={gpuTxt} frac={gpu && gpu.totalMiB > 0 ? gpu.usedMiB / gpu.totalMiB : 0} />
        </div>
      </div>
    </section>
  );
}
