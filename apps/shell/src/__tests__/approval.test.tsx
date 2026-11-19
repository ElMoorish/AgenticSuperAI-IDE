import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApprovalCard } from "../agent/ApprovalCard";
import { handleApplyEdit, type ApplyDeps } from "../agent/applyEdits";
import { BufferManager } from "../editor/models";
import { IpcError } from "../ipc";
import { fm, mkHost } from "./modelsHelpers";

afterEach(cleanup);

describe("ApprovalCard", () => {
  const cmd = (command: string) => ({ id: 1, kind: "command" as const, title: "Run", command });

  it("ApprovalCard_renders_command_line_exactly", () => {
    const tricky = [
      'cl /EHsc /Fe:"out dir\\demo.exe"  main.cpp && demo.exe | findstr "ok"',
      "  leading and trailing spaces  ",
      "echo a;  rm -rf /tmp/x\n# second line",
      "päth\\ünïcode.exe --flag=\"a b\" 'q' `x` $(y) %PATH% <tag>&amp;</tag>",
      "",
    ];
    for (const c of tricky) {
      const { unmount } = render(<ApprovalCard approval={cmd(c)} onResolve={() => {}} />);
      expect(screen.getByTestId("approval-command").textContent).toBe(c); // nothing trimmed, escaped or collapsed
      unmount();
    }
  });

  it("a command card says what it does and answers once", () => {
    const onResolve = vi.fn();
    render(<ApprovalCard approval={cmd("cargo test")} onResolve={onResolve} />);
    expect(screen.getByText(/runs on your computer/i)).toBeTruthy();
    const allow = screen.getByRole("button", { name: "Allow" });
    fireEvent.click(allow);
    fireEvent.click(allow); // double click
    expect(onResolve).toHaveBeenCalledTimes(1);
    expect(onResolve).toHaveBeenCalledWith(true);
    expect((screen.getByRole("button", { name: "Deny" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("an edit card shows the path, +/- counts and the changed lines with accessible add/remove text", () => {
    const onResolve = vi.fn();
    render(
      <ApprovalCard
        approval={{ id: 2, kind: "edit", title: "src/main.cpp", old: "int a;\nint b;\nint c;\n", new: "int a;\nint B;\nint c;\nint d;\n" }}
        onResolve={onResolve}
      />,
    );
    expect(screen.getByText("src/main.cpp")).toBeTruthy();
    expect(screen.getByLabelText("Change summary").textContent).toMatch(/\+2.*−1/);
    const region = screen.getByRole("region", { name: "Changed lines" });
    expect(region.textContent).toContain("removed: int b;");
    expect(region.textContent).toContain("added: int B;");
    fireEvent.click(screen.getByRole("button", { name: "Reject edit" }));
    expect(onResolve).toHaveBeenCalledWith(false);
  });

  it("a very large edit shows a summary instead of rendering the diff", () => {
    const big = Array.from({ length: 5000 }, (_, i) => `row ${i}`).join("\n") + "\n";
    render(<ApprovalCard approval={{ id: 3, kind: "edit", title: "data.csv", old: "", new: big }} onResolve={() => {}} onReview={() => {}} />);
    expect(screen.getByLabelText("Change summary").textContent).toMatch(/Large change \(5,000 lines added, 0 removed\)/);
    expect(screen.queryByRole("region", { name: "Changed lines" })).toBeNull();
    expect(screen.getByRole("button", { name: "Review side by side" })).toBeTruthy();
  });

  it("a new file is labelled as such", () => {
    render(<ApprovalCard approval={{ id: 4, kind: "edit", title: "new.txt", old: "", new: "hi\n" }} onResolve={() => {}} />);
    expect(screen.getByText("New file")).toBeTruthy();
  });
});

describe("handleApplyEdit", () => {
  let h: ReturnType<typeof mkHost>;
  let m: BufferManager;
  let confirm: ReturnType<typeof vi.fn>;
  let deps: ApplyDeps;
  let nextId = 100;
  const ev = (rel: string, old: string, newText: string) => ({ type: "applyEdit" as const, id: nextId++, rel, old, newText });

  beforeEach(() => {
    h = mkHost({ "a.txt": "one\ntwo\n", "crlf.txt": "a\r\nb\r\n" });
    m = new BufferManager(h.host);
    confirm = vi.fn().mockResolvedValue(undefined);
    deps = { models: m, confirm: confirm as unknown as ApplyDeps["confirm"] };
  });

  it("applies the edit to the buffer as one undo step, saves it and confirms success", async () => {
    const e = ev("a.txt", "one\ntwo\n", "one\nTWO\n");
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, true, undefined);
    expect(h.files["a.txt"]).toBe("one\nTWO\n");
    expect(m.getBufferInfo("a.txt")).toMatchObject({ dirty: false, conflict: null });
    const model = fm(m, "a.txt");
    expect(model.undoStack).toHaveLength(1); // Accept lands as one undoable change
    model.undo();
    expect(model.getValue()).toBe("one\ntwo\n");
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(true); // undo goes back past the saved state
  });

  it("keeps CRLF files CRLF", async () => {
    const e = ev("crlf.txt", "a\r\nb\r\n", "a\nB\n"); // host sends EOL-normalised text; the model keeps its own EOL
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, true, undefined);
    expect(h.files["crlf.txt"]).toBe("a\r\nB\r\n");
  });

  it("refuses to touch a buffer with unsaved user edits and tells the agent why", async () => {
    await m.openFile("a.txt");
    fm(m, "a.txt").replaceAll("my own change\n");
    const e = ev("a.txt", "one\ntwo\n", "agent\n");
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, false, expect.stringMatching(/unsaved changes/));
    expect(m.getBuffer("a.txt")).toBe("my own change\n");
    expect(h.files["a.txt"]).toBe("one\ntwo\n");
  });

  it("refuses when the file no longer matches what the agent based its edit on", async () => {
    h.touch("a.txt", "changed by someone else\n");
    const e = ev("a.txt", "one\ntwo\n", "agent\n");
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, false, expect.stringMatching(/changed since the edit was proposed/));
    expect(h.files["a.txt"]).toBe("changed by someone else\n");
  });

  it("a save conflict fails the edit and leaves no half-applied text in the buffer", async () => {
    await m.openFile("a.txt");
    // the disk changes between the check and the save: simulate with a hook on the host
    const realSave = h.host.saveFile;
    h.host.saveFile = async (rel, text, stamp) => {
      h.touch(rel, "sneaky external write\n");
      return realSave(rel, text, stamp);
    };
    const e = ev("a.txt", "one\ntwo\n", "agent\n");
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, false, expect.stringMatching(/changed on disk/));
    expect(h.files["a.txt"]).toBe("sneaky external write\n");
    expect(m.getBuffer("a.txt")).toBe("sneaky external write\n"); // reloaded from disk, agent text dropped
    expect(m.getBufferInfo("a.txt")?.dirty).toBe(false);
  });

  it("a missing or unreadable file fails the edit instead of hanging the agent", async () => {
    const e = ev("gone.txt", "x", "y");
    await handleApplyEdit(e, deps);
    expect(confirm).toHaveBeenCalledWith(e.id, false, expect.any(String));
  });

  it("is idempotent per approval id (a replayed event does not apply twice) and serialises edits", async () => {
    const e = ev("a.txt", "one\ntwo\n", "one\nTWO\n");
    await Promise.all([handleApplyEdit(e, deps), handleApplyEdit(e, deps)]);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(h.saves).toHaveLength(1);
    const e2 = ev("a.txt", "one\nTWO\n", "one\nTWO\nthree\n");
    const e3 = ev("a.txt", "one\nTWO\nthree\n", "one\nTWO\nthree\nfour\n");
    await Promise.all([handleApplyEdit(e2, deps), handleApplyEdit(e3, deps)]);
    expect(h.files["a.txt"]).toBe("one\nTWO\nthree\nfour\n");
    expect(confirm).toHaveBeenCalledTimes(3);
    expect(confirm.mock.calls.every((c) => c[1] === true)).toBe(true);
  });

  it("an unknown_approval answer from the host is ignored quietly", async () => {
    confirm.mockRejectedValue(new IpcError("unknown_approval", "already answered"));
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    await handleApplyEdit(ev("a.txt", "one\ntwo\n", "x\n"), deps);
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });
});
