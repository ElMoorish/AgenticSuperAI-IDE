import type { LocalModel } from "../ipc";

/** "23.4 GB" / "512 MB" (decimal, like file managers and Hugging Face). */
export function fmtSize(bytes: number): string {
  if (!(bytes > 0)) return "0 MB";
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`;
}

export const SOURCE_LABEL: Record<string, string> = { folder: "Folder", ollama: "Ollama", lmstudio: "LM Studio", huggingface: "Hugging Face" };

export interface LaunchForm { port: string; ctx: string; ngl: string; moe: string; alias: string }

/** First problem with the launch form, or null. Mirrors the checks the host makes. */
export function validateLaunch(f: LaunchForm): string | null {
  const int = (s: string) => (/^\d+$/.test(s.trim()) ? Number(s) : NaN);
  const port = int(f.port), ctx = int(f.ctx), ngl = int(f.ngl), moe = int(f.moe);
  if (!(port >= 1024 && port <= 65535)) return "Port must be between 1024 and 65535.";
  if (!(ctx >= 512 && ctx <= 1_048_576)) return "Context size must be between 512 and 1,048,576.";
  if (!(ngl >= 0 && ngl <= 999)) return "GPU layers must be a whole number from 0 to 999.";
  if (!(moe >= 0 && moe <= 999)) return "Expert layers on the CPU must be a whole number from 0 to 999.";
  if (!f.alias.trim()) return "Give the model a name.";
  return null;
}

/** One plain sentence about what the suggested settings mean for this machine. */
export function describeSuggestion(s: { ngl: number; n_cpu_moe: number; fits_gpu: boolean }, vramMib: number | null): string {
  if (vramMib === null) return "No NVIDIA GPU found, so the model will run on the CPU.";
  if (s.fits_gpu) return "Fits in video memory. Everything runs on the GPU.";
  if (s.n_cpu_moe > 0) return `Too big for video memory. The experts of ${s.n_cpu_moe} layers stay in system memory; expect it to be slower.`;
  return `Too big for video memory. ${s.ngl} layers run on the GPU and the rest on the CPU; expect it to be slower.`;
}

/** Name the server reports: the file name without the extension, letters/digits/._:- only. */
export function aliasFor(m: LocalModel): string {
  return m.name.replace(/[^A-Za-z0-9._:-]+/g, "-").replace(/^-+|-+$/g, "") || "model";
}

export const DIRS_KEY = "asa.modelDirs";
export const ENGINE_KEY = "asa.enginePath";
export function loadDirs(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(DIRS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 20) : [];
  } catch {
    return [];
  }
}
export function saveDirs(d: string[]) {
  try { localStorage.setItem(DIRS_KEY, JSON.stringify(d.slice(0, 20))); } catch { /* ignore */ }
}
