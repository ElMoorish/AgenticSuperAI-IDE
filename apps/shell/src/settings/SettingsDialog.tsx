import { useEffect, useRef, useState } from "react";
import { copyDiagnostics, detectServers, getSettings, normalizeError, setSettings, type DetectedServer, type Settings } from "../ipc";
import { clampContext, useAgent } from "../agent/agentStore";
import { useStore } from "../state/store";
import { THEME_LABELS, THEME_NAMES, isThemeName } from "../styles/themes";
import { useModelStatus, validateEndpoint } from "./modelStatus";

const FONT_MIN = 8;
const FONT_MAX = 32;

const DEFAULTS: Settings = { endpoint: "", model: "", api_key: "", ignore: [], theme: "voltage", font_size: 13, recent: [] };

/** Modal settings: model server, model, theme, editor font, ignore globs and context size. */
export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const current = useStore((s) => s.settings.value) ?? DEFAULTS;
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const contextSize = useAgent((s) => s.contextSize);
  const setContextSize = useAgent((s) => s.setContextSize);
  const status = useModelStatus();

  const [endpoint, setEndpoint] = useState(current.endpoint);
  const [model, setModel] = useState(current.model);
  const [apiKey, setApiKey] = useState(current.api_key ?? "");
  const [themeSel, setThemeSel] = useState<string>(theme);
  const [fontSize, setFontSize] = useState(String(current.font_size));
  const [ignore, setIgnore] = useState(current.ignore.join("\n"));
  const [ctx, setCtx] = useState(String(contextSize));
  const [detected, setDetected] = useState<DetectedServer[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [diag, setDiag] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLElement>("input")?.focus();
    return () => prev?.focus?.();
  }, []);

  useEffect(() => {
    let alive = true;
    detectServers().then((d) => { if (alive) setDetected(d); }).catch(() => {});
    return () => { alive = false; };
  }, []);

  // Re-probe (debounced) whenever the endpoint field holds a valid address.
  const endpointError = validateEndpoint(endpoint);
  useEffect(() => {
    if (endpointError) return;
    const t = setTimeout(() => { void useModelStatus.getState().probe(endpoint.trim(), apiKey); }, 400);
    return () => clearTimeout(t);
  }, [endpoint, endpointError, apiKey]);

  const fontNum = Number(fontSize);
  const fontError = !fontSize.trim() || !Number.isFinite(fontNum) || fontNum < FONT_MIN || fontNum > FONT_MAX
    ? `Font size must be a number from ${FONT_MIN} to ${FONT_MAX}` : null;
  const ctxNum = Number(ctx);
  const ctxError = !ctx.trim() || !Number.isFinite(ctxNum) || ctxNum < 2048 ? "Context size must be at least 2048 tokens" : null;
  const invalid = endpointError !== null || fontError !== null || ctxError !== null;

  const modelOptions = status.endpoint === endpoint.trim() ? status.models : [];

  const save = async () => {
    if (invalid || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      let base: Settings = current;
      try { base = await getSettings(); } catch { /* outside the host: use what we have */ }
      const next: Settings = {
        ...base,
        endpoint: endpoint.trim(),
        model: model.trim(),
        api_key: apiKey.trim(),
        font_size: fontNum,
        ignore: ignore.split("\n").map((l) => l.trim()).filter(Boolean),
      };
      let saved: Settings;
      try {
        saved = await setSettings(next);
      } catch (e) {
        const err = normalizeError(e);
        if (err.code === "no_tauri") saved = next; // plain browser: keep in memory only
        else throw e;
      }
      useStore.setState({ settings: { value: saved, loaded: true, error: null } });
      setContextSize(clampContext(ctxNum));
      if (isThemeName(themeSel) && themeSel !== theme) await setTheme(themeSel);
      onClose();
    } catch (e) {
      setSaveError(`Could not save settings: ${normalizeError(e).message}`);
    } finally {
      setSaving(false);
    }
  };

  const copyDiag = async () => {
    try {
      const text = await copyDiagnostics();
      try {
        await navigator.clipboard.writeText(text);
        setDiag("Copied. Paste it into your bug report. It has no file contents, passwords or your user folder.");
      } catch {
        setDiag("Could not reach the clipboard. Try again.");
      }
    } catch (e) {
      setDiag(`Could not build the report: ${normalizeError(e).message}`);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
    else if (e.key === "Tab") {
      const els = [...(dialog.current?.querySelectorAll<HTMLElement>("button:not(:disabled), input, select, textarea") ?? [])];
      if (els.length === 0) return;
      const i = els.indexOf(document.activeElement as HTMLElement);
      const nx = e.shiftKey ? (i <= 0 ? els.length - 1 : i - 1) : i === els.length - 1 ? 0 : i + 1;
      e.preventDefault();
      els[nx].focus();
    }
  };

  return (
    <div className="modal-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div ref={dialog} className="modal settings-modal" role="dialog" aria-modal="true" aria-label="Settings" onKeyDown={onKeyDown}>
        <h3>Settings</h3>

        <label className="sfield">
          <span>Model server address</span>
          <input
            type="text"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            aria-invalid={endpointError !== null}
            aria-describedby="s-endpoint-msg"
            spellCheck={false}
          />
        </label>
        <div id="s-endpoint-msg" className="smsg" role={endpointError ? "alert" : "status"}>
          {endpointError ?? (status.endpoint === endpoint.trim()
            ? status.state === "up" ? `Connected. ${status.models.length} model${status.models.length === 1 ? "" : "s"} available.`
              : status.state === "down" ? `Cannot reach this server: ${status.error ?? "no answer"}` : ""
            : "Checking…")}
        </div>
        {detected.length > 0 && (
          <label className="sfield">
            <span>Servers found on this computer</span>
            <select value="" onChange={(e) => { if (e.target.value) setEndpoint(e.target.value); }}>
              <option value="">Choose one…</option>
              {detected.map((d) => <option key={d.url} value={d.url}>{d.name} ({d.url})</option>)}
            </select>
          </label>
        )}

        <label className="sfield">
          <span>API key (only if the server asks for one)</span>
          <input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} autoComplete="off" spellCheck={false} />
        </label>

        <label className="sfield">
          <span>Model</span>
          {modelOptions.length > 0 ? (
            <select value={model} onChange={(e) => setModel(e.target.value)}>
              {!modelOptions.includes(model) && <option value={model}>{model || "Choose a model…"}</option>}
              {modelOptions.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          ) : (
            <input type="text" value={model} onChange={(e) => setModel(e.target.value)} spellCheck={false} placeholder="model name" />
          )}
        </label>

        <label className="sfield">
          <span>Theme</span>
          <select value={themeSel} onChange={(e) => setThemeSel(e.target.value)}>
            {THEME_NAMES.map((n) => <option key={n} value={n}>{THEME_LABELS[n]}</option>)}
          </select>
        </label>

        <label className="sfield">
          <span>Editor font size</span>
          <input type="number" min={FONT_MIN} max={FONT_MAX} value={fontSize} onChange={(e) => setFontSize(e.target.value)} aria-invalid={fontError !== null} />
        </label>
        {fontError && <div className="smsg" role="alert">{fontError}</div>}

        <label className="sfield">
          <span>Context size (tokens)</span>
          <input type="number" min={2048} step={1024} value={ctx} onChange={(e) => setCtx(e.target.value)} aria-invalid={ctxError !== null} />
        </label>
        {ctxError && <div className="smsg" role="alert">{ctxError}</div>}

        <label className="sfield">
          <span>Hidden from the file tree and the agent (one pattern per line)</span>
          <textarea rows={4} value={ignore} onChange={(e) => setIgnore(e.target.value)} spellCheck={false} className="mono" />
        </label>

        <div className="sfield">
          <span>Problems?</span>
          <button type="button" onClick={() => void copyDiag()}>Copy diagnostics</button>
          {diag && <div className="smsg" role="status">{diag}</div>}
        </div>

        {saveError && <div className="banner err" role="alert"><span>{saveError}</span></div>}
        <div className="modal-actions">
          <button type="button" onClick={onClose}>Cancel</button>
          <button type="button" className="primary" disabled={invalid || saving} onClick={() => void save()}>Save</button>
        </div>
      </div>
    </div>
  );
}
