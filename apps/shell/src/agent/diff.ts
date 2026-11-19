/**
 * Line diff for approval cards. Myers' O(ND) algorithm, so a huge file with a handful of changed
 * lines is cheap, and a rewrite of thousands of lines is detected and reported as "too large"
 * instead of freezing the UI (Review Focus 4).
 */

/** Past this many added + removed lines the card shows a summary, not the diff. */
export const MAX_CHANGED_LINES = 2000;
/** The compact preview shows at most this many lines. */
export const MAX_PREVIEW_LINES = 200;
const CONTEXT = 2;

export type DiffOp = { t: "ctx" | "add" | "del"; text: string };

const lines = (s: string): string[] => {
  if (s === "") return [];
  const n = s.replace(/\r\n/g, "\n");
  const parts = n.split("\n");
  if (parts[parts.length - 1] === "") parts.pop(); // a trailing newline is not an extra empty line
  return parts;
};

/** Edit script a -> b, or null when it needs more than `maxD` inserted + deleted lines. */
function myers(a: string[], b: string[], maxD: number): DiffOp[] | null {
  const n = a.length;
  const m = b.length;
  const max = Math.min(n + m, maxD);
  const off = max + 1;
  const V = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    // Only the band k in [-d-1, d+1] is ever read back, so keep just that part of V.
    trace.push(V.slice(off - d - 1, off + d + 2));
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && V[off + k - 1] < V[off + k + 1]) ? V[off + k + 1] : V[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      V[off + k] = x;
      if (x >= n && y >= m) return backtrack(a, b, trace, d);
    }
  }
  return null;
}

function backtrack(a: string[], b: string[], trace: Int32Array[], dEnd: number): DiffOp[] {
  const ops: DiffOp[] = [];
  let x = a.length;
  let y = b.length;
  for (let d = dEnd; d >= 0; d--) {
    const v = trace[d]; // V before step d, band [-d-1, d+1] stored at index k + d + 1
    const at = (k: number) => v[k + d + 1];
    const k = x - y;
    const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
    const prevX = d === 0 ? 0 : at(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      ops.push({ t: "ctx", text: a[x - 1] });
      x--;
      y--;
    }
    if (d > 0) {
      if (x === prevX) ops.push({ t: "add", text: b[--y] });
      else ops.push({ t: "del", text: a[--x] });
    }
  }
  return ops.reverse();
}

export interface DiffResult {
  ops: DiffOp[];
  added: number;
  removed: number;
  tooLarge: boolean;
}

export function diffLines(oldText: string, newText: string, maxChanged = MAX_CHANGED_LINES): DiffResult {
  const a = lines(oldText);
  const b = lines(newText);
  let lo = 0;
  while (lo < a.length && lo < b.length && a[lo] === b[lo]) lo++;
  let ea = a.length;
  let eb = b.length;
  while (ea > lo && eb > lo && a[ea - 1] === b[eb - 1]) {
    ea--;
    eb--;
  }
  const ma = a.slice(lo, ea);
  const mb = b.slice(lo, eb);
  // Even a perfect match of everything else cannot get below |len difference|.
  if (Math.abs(ma.length - mb.length) > maxChanged) {
    return { ops: [], added: mb.length, removed: ma.length, tooLarge: true };
  }
  const mid = myers(ma, mb, maxChanged);
  if (!mid) return { ops: [], added: mb.length, removed: ma.length, tooLarge: true };
  const ops: DiffOp[] = [
    ...a.slice(0, lo).map((text) => ({ t: "ctx" as const, text })),
    ...mid,
    ...a.slice(ea).map((text) => ({ t: "ctx" as const, text })),
  ];
  let added = 0;
  let removed = 0;
  for (const o of mid) {
    if (o.t === "add") added++;
    else if (o.t === "del") removed++;
  }
  return { ops, added, removed, tooLarge: false };
}

/** Counts only (cheap path for the card header and the "too large" decision). */
export function diffSummary(oldText: string, newText: string): { added: number; removed: number; tooLarge: boolean } {
  const { added, removed, tooLarge } = diffLines(oldText, newText);
  return { added, removed, tooLarge };
}

export interface PreviewLine extends DiffOp {
  /** 1-based line number in the new text (undefined for removed lines). */
  newNo?: number;
}
export type PreviewRow = PreviewLine | { t: "gap"; hidden: number };

/** Changed lines with a little context, capped at MAX_PREVIEW_LINES. */
export function previewRows(ops: DiffOp[]): { rows: PreviewRow[]; hiddenChanges: number } {
  const keep = new Array<boolean>(ops.length).fill(false);
  ops.forEach((o, i) => {
    if (o.t === "ctx") return;
    for (let j = Math.max(0, i - CONTEXT); j <= Math.min(ops.length - 1, i + CONTEXT); j++) keep[j] = true;
  });
  const rows: PreviewRow[] = [];
  let newNo = 0;
  let gap = 0;
  let shown = 0;
  let hiddenChanges = 0;
  ops.forEach((o, i) => {
    if (o.t !== "del") newNo++;
    if (!keep[i]) {
      gap++;
      return;
    }
    if (shown >= MAX_PREVIEW_LINES) {
      if (o.t !== "ctx") hiddenChanges++;
      return;
    }
    if (gap > 0) {
      rows.push({ t: "gap", hidden: gap });
      gap = 0;
    }
    rows.push(o.t === "del" ? { ...o } : { ...o, newNo });
    shown++;
  });
  return { rows, hiddenChanges };
}
