import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DirEntry } from "../ipc";

const fs: Record<string, DirEntry[]> = {};
const listDirMock = vi.fn(async (rel: string) => {
  if (!(rel in fs)) {
    const { IpcError } = await import("../ipc");
    throw new IpcError("not_found", rel);
  }
  return fs[rel];
});
const pickFolderMock = vi.fn(async () => "C:/proj" as string | null);
const openFolderMock = vi.fn(async (path: string) => ({ root: path, name: "proj" }));

vi.mock("../ipc", async (orig) => {
  const real = await orig<typeof import("../ipc")>();
  return { ...real, listDir: (rel: string) => listDirMock(rel), pickFolder: () => pickFolderMock(), openFolder: (p: string) => openFolderMock(p) };
});

import { FileTree } from "../files/FileTree";
import { models } from "../editor/models";
import { dispatchUiEvent, resetUiEventsForTest } from "../events";
import { useStore } from "../state/store";

const dir = (name: string, parent = ""): DirEntry => ({ name, rel: parent ? `${parent}/${name}` : name, isDir: true, size: null });
const file = (name: string, parent = "", size = 10): DirEntry => ({ name, rel: parent ? `${parent}/${name}` : name, isDir: false, size });

const setWorkspace = () => act(() => { useStore.getState().setWorkspace({ root: "C:/proj", name: "proj" }); });
const items = () => screen.queryAllByRole("treeitem");

beforeEach(() => {
  for (const k of Object.keys(fs)) delete fs[k];
  listDirMock.mockClear();
  pickFolderMock.mockClear();
  openFolderMock.mockClear();
  resetUiEventsForTest();
  models.reset();
  act(() => { useStore.getState().setWorkspace(null); });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("FileTree", () => {
  it("shows an empty state with Open folder before a workspace exists", async () => {
    render(<FileTree />);
    expect(screen.getByText("No folder open.")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Open folder" })[0]);
    await waitFor(() => expect(openFolderMock).toHaveBeenCalledWith("C:/proj"));
    await waitFor(() => expect(useStore.getState().workspace.info?.root).toBe("C:/proj"));
  });

  it("windowing renders only visible rows for 10k entries", async () => {
    fs[""] = Array.from({ length: 10_000 }, (_, i) => file(`f${String(i).padStart(5, "0")}.txt`));
    render(<FileTree />);
    setWorkspace();
    await waitFor(() => expect(items().length).toBeGreaterThan(0));
    expect(items().length).toBeLessThan(80);
    const first = items()[0];
    expect(first.getAttribute("aria-setsize")).toBe("10000");
    expect(first.getAttribute("aria-posinset")).toBe("1");
    expect(first.getAttribute("aria-level")).toBe("1");
    expect(screen.getByRole("tree").style.height).not.toBe("");
    // scrolling shifts the window instead of adding rows
    const scroller = screen.getByRole("tree").parentElement!;
    fireEvent.scroll(scroller, { target: { scrollTop: 24 * 5000 } });
    await waitFor(() => expect(items().some((n) => n.getAttribute("data-rel") === "f05000.txt")).toBe(true));
    expect(items().length).toBeLessThan(80);
    expect(items().some((n) => n.getAttribute("data-rel") === "f00001.txt")).toBe(false); // (f00000 stays: it is the roving tab stop)
  });

  it("keyboard navigation moves focus, expands and collapses, and opens files", async () => {
    fs[""] = [dir("src"), dir("docs"), file("README.md")];
    fs["src"] = [file("a.ts", "src", 123)];
    const open = vi.spyOn(models, "openFile").mockResolvedValue();
    render(<FileTree />);
    setWorkspace();
    await waitFor(() => expect(items()).toHaveLength(3));
    const tree = screen.getByRole("tree");
    const tabStops = () => items().filter((n) => n.getAttribute("tabindex") === "0");
    expect(tabStops()).toHaveLength(1);
    expect(tabStops()[0].getAttribute("data-rel")).toBe("src");

    fireEvent.keyDown(tabStops()[0], { key: "ArrowRight" }); // expand src
    await waitFor(() => expect(items()).toHaveLength(4));
    expect(screen.getByRole("treeitem", { name: /src/ }).getAttribute("aria-expanded")).toBe("true");
    expect(listDirMock).toHaveBeenCalledWith("src");

    fireEvent.keyDown(tabStops()[0], { key: "ArrowRight" }); // into the first child
    await waitFor(() => expect(document.activeElement?.getAttribute("data-rel")).toBe("src/a.ts"));
    expect(document.activeElement?.getAttribute("aria-level")).toBe("2");

    fireEvent.keyDown(document.activeElement!, { key: "Enter" }); // open file
    expect(open).toHaveBeenCalledWith("src/a.ts", { size: 123 });

    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" }); // back to parent
    await waitFor(() => expect(document.activeElement?.getAttribute("data-rel")).toBe("src"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" }); // collapse
    await waitFor(() => expect(items()).toHaveLength(3));

    fireEvent.keyDown(document.activeElement!, { key: "End" });
    await waitFor(() => expect(document.activeElement?.getAttribute("data-rel")).toBe("README.md"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    await waitFor(() => expect(document.activeElement?.getAttribute("data-rel")).toBe("docs"));
    fireEvent.keyDown(document.activeElement!, { key: "Home" });
    await waitFor(() => expect(document.activeElement?.getAttribute("data-rel")).toBe("src"));
    expect(tree.getAttribute("role")).toBe("tree");
  });

  it("FsChanged refreshes only affected directories (debounced)", async () => {
    fs[""] = [dir("src"), dir("docs"), dir("lib")];
    fs["src"] = [file("a.ts", "src")];
    fs["docs"] = [file("d.md", "docs")];
    fs["lib"] = [file("l.rs", "lib")];
    render(<FileTree />);
    setWorkspace();
    await waitFor(() => expect(items()).toHaveLength(3));
    for (const name of ["src", "docs"]) fireEvent.click(screen.getByText(name)); // lib stays collapsed
    await waitFor(() => expect(items()).toHaveLength(5));
    await new Promise((r) => setTimeout(r, 20));
    listDirMock.mockClear();

    fs["src"] = [file("a.ts", "src"), file("b.ts", "src")];
    act(() => {
      dispatchUiEvent({ type: "fsChanged", paths: ["src/b.ts", "src/a.ts", "lib/x.rs"], overflow: false });
    });
    expect(listDirMock).not.toHaveBeenCalled(); // debounced
    await waitFor(() => expect(items()).toHaveLength(6));
    await new Promise((r) => setTimeout(r, 300));
    expect(listDirMock.mock.calls.map((c) => c[0])).toEqual(["src"]); // not "", "docs", or the collapsed "lib"
  });

  it("overflow refreshes the root and every expanded directory", async () => {
    fs[""] = [dir("src"), dir("docs")];
    fs["src"] = [file("a.ts", "src")];
    fs["docs"] = [];
    render(<FileTree />);
    setWorkspace();
    await waitFor(() => expect(items()).toHaveLength(2));
    fireEvent.click(screen.getByText("src"));
    await waitFor(() => expect(items()).toHaveLength(3));
    await new Promise((r) => setTimeout(r, 20));
    listDirMock.mockClear();
    act(() => {
      dispatchUiEvent({ type: "fsChanged", paths: [], overflow: true });
    });
    await waitFor(() => expect(listDirMock.mock.calls.length).toBe(2));
    expect(listDirMock.mock.calls.map((c) => c[0]).sort()).toEqual(["", "src"]);
  });

  it("a deleted expanded directory is dropped from the tree", async () => {
    fs[""] = [dir("src")];
    fs["src"] = [file("a.ts", "src")];
    render(<FileTree />);
    setWorkspace();
    await waitFor(() => expect(items()).toHaveLength(1));
    fireEvent.click(screen.getByText("src"));
    await waitFor(() => expect(items()).toHaveLength(2));
    delete fs["src"];
    fs[""] = [];
    act(() => {
      dispatchUiEvent({ type: "fsChanged", paths: ["src"], overflow: false });
    });
    await waitFor(() => expect(items()).toHaveLength(0));
    expect(screen.getByText("This folder is empty.")).toBeTruthy();
  });
});
