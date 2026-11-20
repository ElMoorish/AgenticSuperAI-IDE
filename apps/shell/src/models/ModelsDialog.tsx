import { useCallback, useEffect, useRef, useState } from "react";
import { Boxes, Play, RefreshCw, Square } from "lucide-react";
import {
  detectServers, findEngines, findLocalModels, getSettings, launchModel, modelInfo, modelServerStatus, normalizeError, setSettings, stopModel,
  type DetectedServer, type Engine, type LocalModel, type ModelInfo, type ServerStatus,
} from "../ipc";
import { useStore } from "../state/store";
import { ENGINE_KEY, SOURCE_LABEL, aliasFor, describeSuggestion, fmtSize, loadDirs, saveDirs, validateLaunch } from "./modelsLib";
import { create } from "zustand";

interface D { isOpen: boolean; open: () => void; close: () => void }
export const useModelsDialog = create<D>((set) => ({ isOpen: false, open: () => set({ isOpen: true }), close: () => set({ isOpen: false }) }));

/** Point the IDE at a model server (saves endpoint and model in settings). */
async function connectTo(url: string, model: string) {
  const cur = await getSettings();
  const saved = await setSettings({ ...cur, endpoint: url, model });
  useStore.setState({ settings: { value: saved, loaded: true, error: null } });
}

/** Find models and engines on this PC, start llama-server for one, and connect the IDE to it. */
export function ModelsDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDivElement>(null);
  const [dirs, setDirs] = useState<string[]>(loadDirs);
  const [dirInput, setDirInput] = useState("");
  const [engines, setEngines] = useState<Engine[]>([]);
  const [enginePath, setEnginePath] = useState(() => { try { return localStorage.getItem(ENGINE_KEY) ?? ""; } catch { return ""; } });
  const [models, setModels] = useState<LocalModel[] | null>(null);
  const [servers, setServers] = useState<DetectedServer[]>([]);
  const [scanErr, setScanErr] = useState<string | null>(null);
  const [sel, setSel] = useState<LocalModel | null>(null);
  const [info, setInfo] = useState<ModelInfo | null>(null);
  const [form, setForm] = useState({ port: "8080", ctx: "16384", ngl: "99", moe: "0", alias: "" });
  const [status, setStatus] = useState<ServerStatus | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const connected = useRef<string>("");

  const scan = useCallback(async (extra: string[], explicit: string) => {
    setScanErr(null);
    setModels(null);
    try {
      const [m, e, s] = await Promise.all([findLocalModels(extra), findEngines(explicit || undefined), detectServers().catch(() => [])]);
      setModels(m); setEngines(e); setServers(s);
      // a saved path that is not a real engine (stale or mistyped) is replaced by the first one found
      if (e[0] && !e.some((x) => x.path === explicit)) setEnginePath(e[0].path);
    } catch (e) {
      setModels([]);
      setScanErr(normalizeError(e).message);
    }
  }, []);

  useEffect(() => { void scan(dirs, enginePath); /* once on open */ // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // poll the launched server; connect the IDE the first time it reports ready
  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const st = await modelServerStatus();
        if (!alive) return;
        setStatus(st);
        if (st.state === "ready" && st.port && st.model && connected.current !== `${st.port}/${st.model}`) {
          connected.current = `${st.port}/${st.model}`;
          await connectTo(`http://127.0.0.1:${st.port}`, st.model);
          if (alive) setMsg(`Connected. The IDE now uses ${st.model}.`);
        }
      } catch { /* outside Tauri */ }
    };
    void tick();
    const t = setInterval(() => void tick(), 1000);
    return () => { alive = false; clearInterval(t); };
  }, []);

  const pick = async (m: LocalModel) => {
    setSel(m); setInfo(null); setMsg(null);
    setForm((f) => ({ ...f, alias: aliasFor(m) }));
    try {
      const i = await modelInfo(m.path);
      setInfo(i);
      setForm((f) => ({ ...f, ctx: String(i.suggestion.ctx), ngl: String(i.suggestion.ngl), moe: String(i.suggestion.n_cpu_moe) }));
    } catch (e) {
      setMsg(normalizeError(e).message);
    }
  };

  const addDir = () => {
    const d = dirInput.trim();
    if (!d || dirs.includes(d)) return;
    const next = [...dirs, d];
    setDirs(next); saveDirs(next); setDirInput("");
    void scan(next, enginePath);
  };
  const removeDir = (d: string) => { const next = dirs.filter((x) => x !== d); setDirs(next); saveDirs(next); void scan(next, enginePath); };

  const formError = validateLaunch(form);
  const busy = status?.state === "starting" || status?.state === "ready";
  const canLaunch = !!sel && !!enginePath && !formError && status?.state !== "starting";

  const launch = async () => {
    if (!sel) return;
    setMsg(null);
    try {
      await launchModel({ engine: enginePath, model: sel.path, alias: form.alias.trim(), mmproj: sel.mmproj, ctx: Number(form.ctx), ngl: Number(form.ngl), nCpuMoe: Number(form.moe), port: Number(form.port) });
      connected.current = "";
      try { localStorage.setItem(ENGINE_KEY, enginePath); } catch { /* ignore */ }
    } catch (e) {
      setMsg(normalizeError(e).message);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
  };

  const stateText = status?.state === "starting" ? `Loading ${status.model}… ${status.seconds}s`
    : status?.state === "ready" ? `Running ${status.model} on port ${status.port}`
    : status?.state === "exited" ? `The server stopped${status.exitCode !== null ? ` (exit code ${status.exitCode})` : ""}. See the log below.`
    : "No server started from the IDE.";

  return (
    <div className="modal-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div ref={dialog} className="modal models-modal" role="dialog" aria-modal="true" aria-label="Models" onKeyDown={onKeyDown}>
        <h3><Boxes size={16} aria-hidden="true" /> Models</h3>

        <section className="mrow">
          <label className="sfield">
            <span>Engine (llama-server)</span>
            {engines.length > 0 ? (
              <select value={enginePath} onChange={(e) => setEnginePath(e.target.value)}>
                {!engines.some((x) => x.path === enginePath) && <option value={enginePath}>{enginePath || "Choose one…"}</option>}
                {engines.map((x) => <option key={x.path} value={x.path}>{x.path}</option>)}
              </select>
            ) : (
              <input type="text" value={enginePath} onChange={(e) => setEnginePath(e.target.value)} placeholder="Path to llama-server" spellCheck={false} />
            )}
          </label>
          {engines.length === 0 && <div className="smsg" role="status">llama-server was not found. Type its full path, or the folder that contains it, then press Scan.</div>}
        </section>

        <section className="mrow">
          <div className="sfield">
            <span>Folders with model files (Ollama, LM Studio and the Hugging Face cache are scanned automatically)</span>
            <div className="mdirs">
              {dirs.map((d) => <span key={d} className="mdir">{d}<button type="button" aria-label={`Remove ${d}`} onClick={() => removeDir(d)}>×</button></span>)}
            </div>
            <div className="mpick">
              <input type="text" value={dirInput} onChange={(e) => setDirInput(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addDir(); } }} placeholder="Add a folder path" spellCheck={false} />
              <button type="button" onClick={addDir} disabled={!dirInput.trim()}>Add</button>
              <button type="button" onClick={() => void scan(dirs, enginePath)}><RefreshCw size={14} aria-hidden="true" /> Scan</button>
            </div>
          </div>
        </section>

        <section className="mrow">
          <div className="sfield"><span>Models found</span></div>
          {models === null && <div className="smsg" role="status">Scanning…</div>}
          {scanErr && <div className="smsg" role="alert">{scanErr}</div>}
          {models !== null && models.length === 0 && !scanErr && <div className="smsg" role="status">No .gguf files found. Add the folder that holds them above.</div>}
          <ul className="mlist" role="listbox" aria-label="Models found">
            {(models ?? []).map((m) => (
              <li key={m.path} role="option" aria-selected={sel?.path === m.path}>
                <button type="button" className={sel?.path === m.path ? "sel" : ""} onClick={() => void pick(m)}>
                  <b>{m.name}</b>
                  <span className="mmeta">{fmtSize(m.size_bytes)}{m.quant ? ` · ${m.quant}` : ""}{m.mmproj ? " · vision" : ""}</span>
                  <span className="msrc">{SOURCE_LABEL[m.source] ?? m.source}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>

        {sel && (
          <section className="mrow">
            <div className="smsg" role="status">{info ? describeSuggestion(info.suggestion, info.vramMib) : "Reading the model file…"}</div>
            <div className="mgrid">
              <label className="sfield"><span>Name</span><input type="text" value={form.alias} onChange={(e) => setForm({ ...form, alias: e.target.value })} spellCheck={false} /></label>
              <label className="sfield"><span>Port</span><input type="text" inputMode="numeric" value={form.port} onChange={(e) => setForm({ ...form, port: e.target.value })} /></label>
              <label className="sfield"><span>Context (tokens)</span><input type="text" inputMode="numeric" value={form.ctx} onChange={(e) => setForm({ ...form, ctx: e.target.value })} /></label>
              <label className="sfield"><span>GPU layers</span><input type="text" inputMode="numeric" value={form.ngl} onChange={(e) => setForm({ ...form, ngl: e.target.value })} /></label>
              <label className="sfield"><span>Expert layers on CPU</span><input type="text" inputMode="numeric" value={form.moe} onChange={(e) => setForm({ ...form, moe: e.target.value })} /></label>
            </div>
            {formError && <div className="smsg" role="alert">{formError}</div>}
          </section>
        )}

        {servers.length > 0 && (
          <section className="mrow">
            <div className="sfield"><span>Servers already running</span></div>
            <ul className="mlist">
              {servers.map((s) => (
                <li key={s.url}>
                  <button type="button" onClick={() => void connectTo(s.url, "").then(() => setMsg(`The IDE now uses ${s.name}. Choose its model in Settings.`)).catch((e) => setMsg(normalizeError(e).message))}>
                    <b>{s.name}</b><span className="mmeta">{s.url}</span><span className="msrc">Use</span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}

        <section className="mrow">
          <div className="smsg" role="status">{stateText}</div>
          {msg && <div className="smsg" role="status">{msg}</div>}
          {status && status.log.length > 0 && (status.state === "exited" || status.state === "starting") && (
            <pre className="mlog" aria-label="Server log">{status.log.slice(-14).join("\n")}</pre>
          )}
        </section>

        <div className="modal-actions">
          <button type="button" onClick={onClose}>Close</button>
          {busy && <button type="button" onClick={() => void stopModel().then(() => setMsg("Server stopped."))}><Square size={14} aria-hidden="true" /> Stop server</button>}
          <button type="button" className="primary" disabled={!canLaunch} onClick={() => void launch()}><Play size={14} aria-hidden="true" /> {busy ? "Restart with this model" : "Start server"}</button>
        </div>
      </div>
    </div>
  );
}
