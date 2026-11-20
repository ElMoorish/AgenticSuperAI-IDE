import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { closeTerminal, startUserTerminal, terminalResize, terminalWrite } from "../ipc";
import { onUiEvent } from "../events";
import { useStore } from "../state/store";
import { terminalTheme } from "./terminalTheme";

const enc = new TextEncoder();

/** One PowerShell pty bound to an xterm. xterm answers ConPTY's ESC[6n itself via onData. */
export function UserTerminal({ active }: { active: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const term = new Terminal({
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "Consolas, monospace",
      theme: terminalTheme(),
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
    });
    termRef.current = term;
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    try {
      term.open(el);
    } catch (e) {
      term.dispose();
      setError(e instanceof Error ? e.message : String(e));
      return;
    }
    try { fit.fit(); } catch { /* not laid out yet */ }
    let id: number | null = null;
    let disposed = false;
    let offExit = () => {};

    startUserTerminal(term.rows, term.cols, (bytes) => term.write(new Uint8Array(bytes)))
      .then((tid) => {
        if (disposed) { void closeTerminal(tid).catch(() => {}); return; }
        id = tid;
        offExit = onUiEvent((ev) => {
          if (ev.type === "userTerminalExit" && ev.id === tid) {
            term.write(`\r\n[process exited${ev.code === null ? "" : ` with code ${ev.code}`}]\r\n`);
          }
        });
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));

    const data = term.onData((d) => {
      if (id !== null) void terminalWrite(id, Array.from(enc.encode(d))).catch(() => {});
    });
    const ro = new ResizeObserver(() => {
      if (!el.clientWidth || !el.clientHeight) return;
      try { fit.fit(); } catch { return; }
      if (id !== null) void terminalResize(id, term.rows, term.cols).catch(() => {});
    });
    ro.observe(el);
    return () => {
      disposed = true;
      ro.disconnect();
      data.dispose();
      offExit();
      if (id !== null) void closeTerminal(id).catch(() => {});
      term.dispose();
    };
  }, []);

  // follow the app theme: colours are read from the live CSS variables
  const appTheme = useStore((s) => s.theme);
  useEffect(() => {
    if (termRef.current) {
      termRef.current.options.theme = terminalTheme();
      termRef.current.options.fontFamily = getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "Consolas, monospace";
    }
  }, [appTheme]);

  useEffect(() => {
    if (active) { try { fitRef.current?.fit(); } catch { /* hidden */ } }
  }, [active]);

  return error
    ? <div className="banner err" role="alert"><span>The terminal could not start: {error}</span></div>
    : <div ref={host} className="term-host" aria-label="Terminal" />;
}
