import { describe, expect, it } from "vitest";
import { diffLines, diffSummary, MAX_CHANGED_LINES, MAX_PREVIEW_LINES, previewRows } from "../agent/diff";

const nl = (n: number, prefix = "line") => Array.from({ length: n }, (_, i) => `${prefix} ${i}`).join("\n") + "\n";

describe("diffSummary", () => {
  it("counts added and removed lines", () => {
    expect(diffSummary("a\nb\nc\n", "a\nB\nc\nd\n")).toEqual({ added: 2, removed: 1, tooLarge: false });
    expect(diffSummary("a\n", "a\n")).toEqual({ added: 0, removed: 0, tooLarge: false });
    expect(diffSummary("", "x\ny\n")).toEqual({ added: 2, removed: 0, tooLarge: false }); // new file
    expect(diffSummary("x\ny\n", "")).toEqual({ added: 0, removed: 2, tooLarge: false });
  });

  it("ignores EOL style and a trailing newline when comparing", () => {
    expect(diffSummary("a\r\nb\r\n", "a\nb\n")).toEqual({ added: 0, removed: 0, tooLarge: false });
    expect(diffSummary("a\nb", "a\nb\n")).toEqual({ added: 0, removed: 0, tooLarge: false });
  });

  it("diffSummary_flags_too_large", () => {
    const huge = diffSummary("", nl(MAX_CHANGED_LINES + 1));
    expect(huge.tooLarge).toBe(true);
    const rewrite = diffSummary(nl(3000, "old"), nl(3000, "new"));
    expect(rewrite.tooLarge).toBe(true);
    const justUnder = diffSummary("", nl(MAX_CHANGED_LINES));
    expect(justUnder).toEqual({ added: MAX_CHANGED_LINES, removed: 0, tooLarge: false });
  });

  it("a huge file with a few changed lines is NOT too large and is fast", () => {
    const big = nl(50_000);
    const edited = big.replace("line 0\n", "FIRST\n").replace("line 49999\n", "LAST\n").replace("line 25000\n", "MID\n");
    const t0 = performance.now();
    const r = diffSummary(big, edited);
    expect(r).toEqual({ added: 3, removed: 3, tooLarge: false });
    expect(performance.now() - t0).toBeLessThan(2000);
  });
});

describe("diffLines", () => {
  const apply = (ops: ReturnType<typeof diffLines>["ops"]) => ({
    old: ops.filter((o) => o.t !== "add").map((o) => o.text),
    new: ops.filter((o) => o.t !== "del").map((o) => o.text),
  });

  it("produces a script that reproduces both sides exactly", () => {
    const cases: [string, string][] = [
      ["a\nb\nc\nd\ne\n", "a\nc\nX\ne\nf\n"],
      ["x\n", "y\n"],
      ["1\n2\n3\n", "3\n2\n1\n"],
      ["", "only\n"],
      ["same\nsame\nsame\n", "same\nsame\n"],
      ["a\nb\nc\n", "b\n"],
    ];
    for (const [o, n] of cases) {
      const r = diffLines(o, n);
      expect(r.tooLarge).toBe(false);
      const a = apply(r.ops);
      expect(a.old.join("\n")).toBe(o.replace(/\n$/, ""));
      expect(a.new.join("\n")).toBe(n.replace(/\n$/, ""));
    }
  });

  it("random edits always reproduce both sides (property check)", () => {
    let seed = 7;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    for (let round = 0; round < 60; round++) {
      const a = Array.from({ length: Math.floor(rnd() * 30) }, () => String(Math.floor(rnd() * 6)));
      const b = a.filter(() => rnd() > 0.3);
      for (let i = 0; i < 5; i++) b.splice(Math.floor(rnd() * (b.length + 1)), 0, String(Math.floor(rnd() * 9)));
      const r = diffLines(a.join("\n"), b.join("\n"));
      const got = apply(r.ops);
      expect(got.old).toEqual(a);
      expect(got.new).toEqual(b);
    }
  });
});

describe("previewRows", () => {
  it("shows changed lines with context, collapses the rest and numbers new lines", () => {
    const r = diffLines(nl(30), nl(30).replace("line 15\n", "CHANGED\n"));
    const { rows, hiddenChanges } = previewRows(r.ops);
    expect(hiddenChanges).toBe(0);
    expect(rows[0]).toEqual({ t: "gap", hidden: 13 });
    const del = rows.find((x) => x.t === "del");
    const add = rows.find((x) => x.t === "add");
    expect(del && "text" in del && del.text).toBe("line 15");
    expect(add && "newNo" in add && add.newNo).toBe(16);
    expect(rows.filter((x) => x.t === "ctx")).toHaveLength(4);
  });

  it("caps the preview and reports how many changes it hid", () => {
    const r = diffLines("", nl(1000));
    const { rows, hiddenChanges } = previewRows(r.ops);
    expect(rows.length).toBeLessThanOrEqual(MAX_PREVIEW_LINES + 1);
    expect(hiddenChanges).toBe(1000 - MAX_PREVIEW_LINES);
  });
});
