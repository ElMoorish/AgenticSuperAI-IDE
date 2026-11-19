/** Real Monaco adapter for `ModelLike` plus editor creation. Lazy-loaded; never imported by tests. */
import { monaco, setupMonaco } from "./monacoSetup";
import type { Eol, ModelLike } from "./models";

export class MonacoModel implements ModelLike {
  constructor(readonly raw: monaco.editor.ITextModel) {}
  getValue() { return this.raw.getValue(); }
  getEOL() { return this.raw.getEOL(); }
  setEOL(e: Eol) {
    this.raw.setEOL(e === "\r\n" ? monaco.editor.EndOfLineSequence.CRLF : monaco.editor.EndOfLineSequence.LF);
  }
  getAlternativeVersionId() { return this.raw.getAlternativeVersionId(); }
  pushStackElement() { this.raw.pushStackElement(); }
  /**
   * One edit covering only the changed middle (common prefix/suffix trimmed) so the cursor,
   * scroll position and decorations outside the change survive an agent edit.
   */
  replaceAll(text: string) {
    const m = this.raw;
    const next = text.replace(/\r\n|\r|\n/g, m.getEOL());
    const cur = m.getValue();
    let a = 0;
    const max = Math.min(cur.length, next.length);
    while (a < max && cur.charCodeAt(a) === next.charCodeAt(a)) a++;
    let b = 0;
    while (b < max - a && cur.charCodeAt(cur.length - 1 - b) === next.charCodeAt(next.length - 1 - b)) b++;
    if (a > 0 && cur.charCodeAt(a - 1) === 13) a--; // never split a CRLF pair
    if (b > 0 && cur.charCodeAt(cur.length - b) === 10 && cur.charCodeAt(cur.length - b - 1) === 13) b--;
    if (cur === next) return;
    const from = m.getPositionAt(a);
    const to = m.getPositionAt(cur.length - b);
    m.pushEditOperations([], [{ range: new monaco.Range(from.lineNumber, from.column, to.lineNumber, to.column), text: next.slice(a, next.length - b) }], () => null);
  }
  reload(text: string) { this.raw.setValue(text); }
  onDidChangeContent(cb: () => void) { return this.raw.onDidChangeContent(cb); }
  dispose() { this.raw.dispose(); }
}

export function createMonacoModel(rel: string, text: string, language: string, _readOnly: boolean): ModelLike {
  setupMonaco();
  const uri = monaco.Uri.from({ scheme: "file", path: "/" + rel });
  monaco.editor.getModel(uri)?.dispose();
  return new MonacoModel(monaco.editor.createModel(text, language, uri));
}

export type CodeEditor = monaco.editor.IStandaloneCodeEditor;

export function createEditor(container: HTMLElement, opts: { theme: string; fontSize: number }): CodeEditor {
  setupMonaco();
  return monaco.editor.create(container, {
    model: null,
    theme: opts.theme,
    automaticLayout: true,
    fontSize: opts.fontSize,
    fontFamily: '"Spline Sans Mono", Consolas, monospace',
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    renderWhitespace: "selection",
    smoothScrolling: false,
    "semanticHighlighting.enabled": false,
  });
}

/** Read-only side-by-side diff for reviewing an agent edit. */
export function createDiffViewer(
  container: HTMLElement,
  o: { theme: string; fontSize: number; original: string; modified: string; language: string },
): { dispose(): void } {
  setupMonaco();
  const original = monaco.editor.createModel(o.original, o.language);
  const modified = monaco.editor.createModel(o.modified, o.language);
  const ed = monaco.editor.createDiffEditor(container, {
    theme: o.theme,
    automaticLayout: true,
    readOnly: true,
    originalEditable: false,
    renderSideBySide: true,
    fontSize: o.fontSize,
    fontFamily: '"Spline Sans Mono", Consolas, monospace',
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
  });
  ed.setModel({ original, modified });
  return {
    dispose() {
      ed.dispose();
      original.dispose();
      modified.dispose();
    },
  };
}

export { monaco };
