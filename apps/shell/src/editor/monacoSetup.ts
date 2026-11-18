/**
 * Monaco bootstrap (lazy-imported from the editor). CSP-safe: workers come from Vite `?worker`
 * imports (same-origin / blob:, allowed by `worker-src 'self' blob:`), no eval, no CDN.
 * Only the editor, json, css, html and ts/js workers are shipped.
 */
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import JsonWorker from "monaco-editor/language/json/json.worker.js?worker";
import CssWorker from "monaco-editor/language/css/css.worker.js?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker.js?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker.js?worker";
import { THEME_NAMES, type ThemeName } from "../styles/themes";
import { buildMonacoTheme, monacoThemeName } from "./monacoThemes";

export { monaco };

let ready = false;

export function setupMonaco(): typeof monaco {
  if (ready) return monaco;
  ready = true;
  (self as unknown as { MonacoEnvironment: monaco.Environment }).MonacoEnvironment = {
    getWorker(_id: string, label: string): Worker {
      switch (label) {
        case "json": return new JsonWorker();
        case "css": case "scss": case "less": return new CssWorker();
        case "html": case "handlebars": case "razor": return new HtmlWorker();
        case "typescript": case "javascript": return new TsWorker();
        default: return new EditorWorker();
      }
    },
  };
  for (const t of THEME_NAMES) monaco.editor.defineTheme(monacoThemeName(t), buildMonacoTheme(t));
  // No project/tsconfig is loaded into the in-browser TS service, so semantic errors would be noise
  // (e.g. "cannot find name console"); keep syntax checking only.
  const quiet = { noSemanticValidation: true, noSyntaxValidation: false, noSuggestionDiagnostics: true };
  monaco.typescript.typescriptDefaults.setDiagnosticsOptions(quiet);
  monaco.typescript.javascriptDefaults.setDiagnosticsOptions(quiet);
  return monaco;
}

export function applyMonacoTheme(t: ThemeName): void {
  setupMonaco();
  monaco.editor.setTheme(monacoThemeName(t));
}
