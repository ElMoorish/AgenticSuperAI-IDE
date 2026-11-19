import { Fragment, memo, type ReactNode } from "react";

/**
 * A small, safe Markdown renderer for the agent's answers: fenced code, headings, lists, paragraphs,
 * and inline `code`, **bold**, *italic*. It builds React elements only (no HTML is ever parsed), links are
 * shown as plain text, and unfinished syntax while a reply is still streaming stays literal.
 */

const INLINE = /(`[^`\n]+`|\*\*[^*\n]+?\*\*|\*[^*\s][^*\n]*?\*)/g;

export function renderInline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let n = 0;
  for (const m of text.matchAll(INLINE)) {
    const i = m.index ?? 0;
    if (i > last) out.push(text.slice(last, i));
    const tok = m[0];
    if (tok.startsWith("`")) out.push(<code key={n++}>{tok.slice(1, -1)}</code>);
    else if (tok.startsWith("**")) out.push(<strong key={n++}>{tok.slice(2, -2)}</strong>);
    else out.push(<em key={n++}>{tok.slice(1, -1)}</em>);
    last = i + tok.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

type Block =
  | { t: "code"; lang: string; text: string }
  | { t: "h"; level: number; text: string }
  | { t: "ul" | "ol"; items: string[] }
  | { t: "p"; text: string };

export function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;
  const startsBlock = (l: string) => /^(```|#{1,4}\s|\s*[-*]\s+|\s*\d+[.)]\s+)/.test(l);
  while (i < lines.length) {
    const line = lines[i];
    const fence = /^```\s*([\w+#.-]*)\s*$/.exec(line);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) body.push(lines[i++]);
      i++; // the closing fence (absent while streaming: the block simply runs to the end)
      blocks.push({ t: "code", lang: fence[1], text: body.join("\n") });
      continue;
    }
    const h = /^(#{1,4})\s+(.*)$/.exec(line);
    if (h) {
      blocks.push({ t: "h", level: h[1].length, text: h[2] });
      i++;
      continue;
    }
    const ul = /^\s*[-*]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (ul || ol) {
      const kind = ul ? "ul" : "ol";
      const re = ul ? /^\s*[-*]\s+(.*)$/ : /^\s*\d+[.)]\s+(.*)$/;
      const items: string[] = [];
      while (i < lines.length) {
        const m = re.exec(lines[i]);
        if (!m) break;
        items.push(m[1]);
        i++;
      }
      blocks.push({ t: kind, items });
      continue;
    }
    if (line.trim() === "") {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() !== "" && (para.length === 0 || !startsBlock(lines[i]))) para.push(lines[i++]);
    blocks.push({ t: "p", text: para.join("\n") });
  }
  return blocks;
}

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const blocks = parseBlocks(text);
  return (
    <div className="md">
      {blocks.map((b, k) => {
        switch (b.t) {
          case "code":
            return (
              <pre key={k} className="mono" data-lang={b.lang || undefined}>
                <code>{b.text}</code>
              </pre>
            );
          case "h":
            return <div key={k} className={`md-h md-h${b.level}`} role="heading" aria-level={b.level + 1}>{renderInline(b.text)}</div>;
          case "ul":
            return <ul key={k}>{b.items.map((it, j) => <li key={j}>{renderInline(it)}</li>)}</ul>;
          case "ol":
            return <ol key={k}>{b.items.map((it, j) => <li key={j}>{renderInline(it)}</li>)}</ol>;
          default:
            return (
              <p key={k}>
                {b.text.split("\n").map((ln, j) => (
                  <Fragment key={j}>
                    {j > 0 && <br />}
                    {renderInline(ln)}
                  </Fragment>
                ))}
              </p>
            );
        }
      })}
    </div>
  );
});
