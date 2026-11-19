import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { timeline } from "../agent/AgentPanel";
import { Markdown, parseBlocks } from "../agent/Markdown";
import type { Message, Step } from "../agent/agentStore";

const msg = (id: number, role: Message["role"] = "assistant"): Message => ({ id, role, text: `m${id}` });
const step = (seq: number): Step => ({ id: `t${seq}`, title: `s${seq}`, kind: "read", status: "done", seq });

describe("timeline", () => {
  it("interleaves messages and steps in arrival order and groups consecutive steps", () => {
    const t = timeline([msg(1, "user"), msg(2), msg(5)], [step(3), step(4), step(6)]);
    expect(t.map((x) => (x.kind === "msg" ? `m${x.msg.id}` : `steps[${x.steps.map((s) => s.seq)}]`))).toEqual([
      "m1", "m2", "steps[3,4]", "m5", "steps[6]",
    ]);
  });
  it("handles empty input", () => {
    expect(timeline([], [])).toEqual([]);
    expect(timeline([msg(1)], [])).toHaveLength(1);
    expect(timeline([], [step(1)])).toHaveLength(1);
  });
});

describe("Markdown", () => {
  it("renders bold, italic and inline code", () => {
    const { container } = render(<Markdown text={"This is **bold**, *soft* and `code`."} />);
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("em")?.textContent).toBe("soft");
    expect(container.querySelector("code")?.textContent).toBe("code");
    expect(container.textContent).not.toContain("**");
  });

  it("renders lists, headings and fenced code", () => {
    const text = "# Title\n\n- one\n- two\n\n1. first\n2. second\n\n```python\nprint('x')\n```\n";
    const { container } = render(<Markdown text={text} />);
    expect(container.querySelector("[role=heading]")?.textContent).toBe("Title");
    expect(container.querySelectorAll("ul li")).toHaveLength(2);
    expect(container.querySelectorAll("ol li")).toHaveLength(2);
    expect(container.querySelector("pre code")?.textContent).toBe("print('x')");
  });

  it("never turns text into HTML", () => {
    const { container } = render(<Markdown text={'<img src=x onerror="alert(1)"> and <script>alert(2)</script>'} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("<script>");
  });

  it("leaves unfinished syntax literal while a reply is streaming", () => {
    const { container } = render(<Markdown text={"half **bold and `code"} />);
    expect(container.textContent).toBe("half **bold and `code");
    // an unclosed code fence runs to the end instead of swallowing nothing
    expect(parseBlocks("```js\nlet a = 1;")).toEqual([{ t: "code", lang: "js", text: "let a = 1;" }]);
  });

  it("keeps single line breaks inside a paragraph and does not treat snake_case or 2 * 3 as emphasis", () => {
    const { container } = render(<Markdown text={"line one\nline two\n\nuse my_var_name and 2 * 3 * 4"} />);
    expect(container.querySelectorAll("p")).toHaveLength(2);
    expect(container.querySelectorAll("br")).toHaveLength(1);
    expect(container.querySelector("em")).toBeNull();
  });
});
