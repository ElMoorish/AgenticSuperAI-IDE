import { useEffect, useRef, useState } from "react";
import { recoveryClear, type RecoveredBuffer } from "../ipc";
import { useStore } from "../state/store";
import { models } from "./models";
import { RecoverySync, defaultRecoveryDeps, listRecovered, restoreBuffers } from "./recovery";

/** Keeps unsaved text safe and offers it back after a crash. Renders nothing when there is none. */
export function RecoveryBanner() {
  const root = useStore((s) => s.workspace.info?.root);
  const [found, setFound] = useState<RecoveredBuffer[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  const sync = useRef<RecoverySync | null>(null);

  useEffect(() => {
    if (!root) return;
    const s = new RecoverySync(defaultRecoveryDeps(models));
    s.paused = true; // do not touch old snapshots until the user has decided
    sync.current = s;
    let alive = true;
    void listRecovered().then((l) => {
      if (!alive) return;
      setFound(l);
      if (l.length === 0) s.paused = false;
    });
    s.start();
    return () => {
      alive = false;
      s.stop();
      sync.current = null;
      setFound([]);
    };
  }, [root]);

  if (found.length === 0 && !msg) return null;
  const done = () => { setFound([]); if (sync.current) { sync.current.reset(); sync.current.paused = false; } };
  return (
    <div className="banner warn" role="alert">
      {found.length > 0 ? (
        <>
          <span>
            Unsaved changes from your last session were found for {found.length} file{found.length === 1 ? "" : "s"}
            {found.length <= 3 ? `: ${found.map((f) => f.rel).join(", ")}` : ""}.
          </span>
          <button
            type="button"
            className="primary"
            onClick={() => void restoreBuffers(models, found).then((r) => { setMsg(`Restored ${r.length} file${r.length === 1 ? "" : "s"}. Review and save them.`); done(); })}
          >Restore</button>
          <button type="button" onClick={() => { void Promise.all(found.map((f) => recoveryClear(f.rel).catch(() => {}))).then(done); }}>Discard</button>
        </>
      ) : (
        <>
          <span>{msg}</span>
          <button type="button" onClick={() => setMsg(null)}>Dismiss</button>
        </>
      )}
    </div>
  );
}
