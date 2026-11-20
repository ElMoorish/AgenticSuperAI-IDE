/**
 * ASA_DEV_MOCK_HOST: dev-only stand-in for the Tauri host so the UI can be exercised in a plain
 * browser (`npm run dev`). Installed from main.tsx only when `import.meta.env.DEV && !isTauri()`;
 * Vite drops the dynamic import from production builds (a test checks dist for this marker).
 * It fakes just enough of window.__TAURI_INTERNALS__ for invoke() and Channel.
 */
interface MockFile { text: string; v: number }
type Msg = Record<string, unknown>;

const files = new Map<string, MockFile>();
const dirsOf = (rel: string) => rel.split("/").slice(0, -1).reduce<string[]>((a, _s, i, p) => [...a, p.slice(0, i + 1).join("/")], []);

function seed() {
  const add = (rel: string, text: string) => files.set(rel, { text, v: 1 });
  add("README.md", "# Demo project\n\nA fake in-memory workspace served by the dev mock host.\n");
  add("src/main.ts", "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nconst msg: string = \"hello\";\nconsole.log(add(1, 2), msg);\n");
  add("src/util.ts", "export const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));\n");
  add("src/styles.css", "body {\n  margin: 0;\n  color: #333;\n}\n");
  add("data/config.json", "{\n  \"name\": \"demo\",\n  \"version\": 1\n}\n");
  add("windows.txt", "line one\r\nline two\r\nline three\r\n");
  add("big.log", "x".repeat(6 * 1024 * 1024));
  add("image.png", "\u0000binary");
  for (let i = 0; i < 300; i++) add(`many/file${String(i).padStart(3, "0")}.txt`, `file ${i}\n`);
}

const hash = (f: MockFile) => `${f.text.length}:${f.v}`;
const stampOf = (f: MockFile) => ({ mtimeMs: f.v, hash: hash(f) });
const err = (code: string, message: string) => ({ code, message });

export function installMockHost(): void {
  seed();
  const callbacks = new Map<number, (m: unknown) => void>();
  let nextCb = 1;
  const sinks: { id: number; n: number }[] = [];
  const emit = (ev: Msg) => sinks.forEach((s) => callbacks.get(s.id)?.({ message: ev, index: s.n++ }));
  const settings = { endpoint: "http://localhost:8080/v1", model: "", ignore: [], theme: "voltage", font_size: 13, recent: ["C:/demo"] };

  const list = (rel: string) => {
    const prefix = rel ? rel + "/" : "";
    const seen = new Map<string, { name: string; rel: string; isDir: boolean; size: number | null }>();
    for (const [p, f] of files) {
      if (!p.startsWith(prefix)) continue;
      const rest = p.slice(prefix.length);
      const i = rest.indexOf("/");
      if (i < 0) seen.set(rest, { name: rest, rel: p, isDir: false, size: f.text.length });
      else seen.set(rest.slice(0, i), { name: rest.slice(0, i), rel: prefix + rest.slice(0, i), isDir: true, size: null });
    }
    if (!seen.size && rel && ![...files.keys()].some((p) => p.startsWith(prefix))) throw err("not_found", rel);
    return [...seen.values()].sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name));
  };

  const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
    get_settings: () => settings,
    find_local_models: () => [
      { name: "Qwen3.6-35B-A3B-Q4_K_P", path: "demo/qwen36.gguf", size_bytes: 23e9, quant: "Q4_K_P", source: "folder", mmproj: "demo/mmproj.gguf", kind: "gguf" },
      { name: "llama3:8b", path: "demo/blob", size_bytes: 4.7e9, quant: null, source: "ollama", mmproj: null, kind: "gguf" },
      { name: "Qwen3.8-40B", path: "demo/q40", size_bytes: 79.9e9, quant: null, source: "folder", mmproj: null, kind: "safetensors" },
    ],
    find_airllm: () => ({ python: "E:/airllm-test/venv/Scripts/python.exe", server: "E:/repo/tools/airllm-server/server.py" }),
    find_engines: () => [{ name: "llama-server", path: "C:/llama.cpp/llama-server.exe" }],
    model_info: () => ({ info: { block_count: 40, expert_count: 128, context_length: 262144 }, suggestion: { ngl: 99, n_cpu_moe: 28, ctx: 16384, fits_gpu: false }, vramMib: 8188 }),
    launch_model: () => null,
    stop_model: () => null,
    model_server_status: () => ({ state: "stopped", model: null, port: null, seconds: 0, exitCode: null, log: [] }),
    set_settings: (a) => Object.assign(settings, a.settings),
    pick_folder: () => "C:/demo",
    open_folder: (a) => ({ root: String(a.path), name: "demo" }),
    list_dir: (a) => list(String(a.rel)),
    read_file: (a) => {
      const f = files.get(String(a.rel));
      if (!f) throw err("not_found", String(a.rel));
      if (f.text === "\u0000binary") throw err("binary", "binary file");
      if (f.text.length > 5 * 1024 * 1024 && String(a.rel) !== "big.log") throw err("too_large", "too large");
      return { text: f.text, stamp: stampOf(f) };
    },
    save_file: (a) => {
      const rel = String(a.rel);
      const f = files.get(rel);
      if (!f) throw err("not_found", rel);
      if ((a.stamp as { hash: string }).hash !== hash(f)) throw err("conflict", "file changed on disk");
      f.text = String(a.text);
      f.v++;
      setTimeout(() => emit({ type: "fsChanged", paths: [rel], overflow: false }), 30);
      return stampOf(f);
    },
    subscribe_events: (a) => {
      const id = (a.onEvent as { id: number }).id;
      sinks.push({ id, n: 0 });
      return null;
    },
  };

  const w = window as unknown as Record<string, unknown>;
  w.__TAURI_INTERNALS__ = {
    transformCallback: (cb: (m: unknown) => void) => {
      const id = nextCb++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback: (id: number) => void callbacks.delete(id),
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
      const h = handlers[cmd];
      if (!h) throw err("unknown", `mock host: ${cmd} not implemented`);
      return h(args);
    },
  };
  // handle for manual / scripted checks from the console
  w.__asaMock = {
    /** Simulate an external editor writing a file. */
    touch(rel: string, text: string) {
      const f = files.get(rel) ?? { text: "", v: 0 };
      f.text = text;
      f.v++;
      files.set(rel, f);
      emit({ type: "fsChanged", paths: [rel], overflow: false });
    },
    remove(rel: string) {
      files.delete(rel);
      emit({ type: "fsChanged", paths: [rel], overflow: false });
    },
    read: (rel: string) => files.get(rel)?.text,
    dirs: (rel: string) => dirsOf(rel),
  };
}
