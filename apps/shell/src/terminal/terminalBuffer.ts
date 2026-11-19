/** Bounded text buffer for agent terminal output: keeps the newest `cap` characters. */
export class TerminalBuffer {
  private parts: string[] = [];
  private len = 0;
  dropped = false;
  constructor(private readonly cap = 200_000) {}

  append(text: string): void {
    if (!text) return;
    this.parts.push(text);
    this.len += text.length;
    while (this.len > this.cap && this.parts.length > 0) {
      const over = this.len - this.cap;
      const first = this.parts[0];
      this.dropped = true;
      if (first.length <= over) {
        this.parts.shift();
        this.len -= first.length;
      } else {
        this.parts[0] = first.slice(over);
        this.len -= over;
      }
    }
  }
  get length(): number { return this.len; }
  text(): string { return this.parts.join(""); }
}

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;
/** Agent output is shown as plain text: escape sequences are removed, never interpreted. */
export function stripAnsi(s: string): string {
  return s.replace(ANSI, "").replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}
