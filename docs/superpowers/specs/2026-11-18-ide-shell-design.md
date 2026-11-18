# IDE Shell (Project 1 of 4) - Design

Status: draft for approval. Date: 2026-11-18.

The four projects, built one at a time in this order: (1) IDE shell, (2) build/run + context-safe digests, (3) desktop control of any app, (4) model manager (downloads to E:, AirLLM batch). This document covers project 1 only.

## 1. Intent

A native Windows desktop IDE for local agentic coding. The user edits code in a Monaco editor and works with an agent (`agent-core`, already built and tested) that runs against a local model server (Ollama, LM Studio, llama-server, Unsloth Studio if it exposes an OpenAI-compatible API - unverified). The shell is the "editor" in the ACP sense: it owns files, terminals, permission prompts and the chat view. The agent never touches disk or processes directly.

Why our own shell and not the VS Code extension (kept only as a dev harness): the agent UI, approval flow, model/VRAM awareness and later desktop control have to be first-class, and the product must stay small and fast on 16 GB RAM with the model competing for memory.

## 2. Goals

- Open a folder; browse, edit, save files in Monaco with tabs, dirty state, find/replace, undo.
- Chat with the agent in Plan and Build modes; see streamed text and structured tool calls; stop a running turn.
- Every agent edit is shown as a diff and needs Accept/Reject; Accept lands as a normal undoable editor change.
- Integrated terminal (user shell) plus read-only views of terminals the agent starts.
- Configure and health-check the model endpoint; show model name, tokens/s, estimated context use and GPU memory.
- Robust: no data loss, no hangs, recoverable from agent or model-server crashes.
- Efficient: low idle memory, fast startup, no UI jank while the agent streams.

## 3. Non-goals (v1)

Extensions/plugins, language servers (Monaco built-in language features only), debugger, git UI, remote development, multi-window, macOS/Linux builds (code stays portable but only Windows is tested), build/run digests (project 2), desktop control (project 3), model downloads (project 4).

## 4. Anti-goals

- The webview never gets a general shell or file API. A compromised page or model output must not be able to run commands or read outside the workspace.
- Agent output must never be able to freeze the UI (unbounded streams, huge diffs, giant files).
- No silent overwrite of a file the user changed elsewhere.
- No feature that only works with one model server.

## 5. Constraints

Windows 10, 16 GB RAM, RTX 4060 8 GB (the model needs most of it). WebView2 runtime required (Tauri); installer must bootstrap it. `agent-core` is Rust, speaks line-delimited JSON-RPC (ACP subset) on stdio. No git repo exists yet.

## 6. Approaches considered

| Approach | Trade-off | Verdict |
|---|---|---|
| A. Tauri v2 + Monaco; UI, host and `agent-core` as separate processes | Crash isolation, agent reusable by other editors; extra IPC hop | **Chosen** (approved by the user) |
| B. Embed `agent-core` as a library in the Tauri process | Fastest, simplest; a hang or panic in the agent can freeze or kill the IDE | Rejected |
| C. Webview-only app talking to the agent over a local WebSocket | Easy to test in a browser; opens a network port and weakens the sandbox | Rejected |

## 7. Architecture

```
 WebView (React + TS)              Tauri Rust process                   Sidecar
 Monaco, xterm.js, chat   <-IPC->  ide-host crate (ACP editor side)  <-stdio->  agent-core
                                   fs sandbox, pty, watcher, settings            (ACP)
                                                   |
                                                   +--> HTTP --> Ollama / LM Studio / llama-server
```

Layout: Cargo workspace gains `crates/ide-host` (all logic, no Tauri dependency, unit-testable) and `apps/shell` (Tauri app: thin commands plus the React frontend). `extension/` stays as a harness.

### 7.1 ide-host (Rust)

- **Agent supervisor:** spawns `agent-core` with `tokio::process`, not through the JS shell plugin, so the webview has no `shell:allow-execute` permission at all. Reads stdout lines, routes responses/notifications, answers agent-initiated requests. Restarts on crash with backoff and tells the UI; the session transcript is persisted so a restart can resume.
- **ACP editor side:** implements `fs/read_text_file`, `fs/write_text_file`, `terminal/*`, `session/request_permission`.
  - `fs/read_text_file` returns the *open buffer* content (including unsaved edits) when the file is open in the UI, else disk content.
  - Writes go through the UI as a pending edit (diff view); on Accept the UI applies it with `executeEdits` (undoable) and saves. Files not open are written atomically (temp file + rename) after Accept.
- **Workspace sandbox:** every path is canonicalised and must stay under the opened root (rejects `..`, junctions and symlinks that escape). Applies to UI commands and agent requests alike.
- **Conflict guard:** each open file records mtime+hash at load; save or agent write is refused with a prompt if the disk copy changed since.
- **File watcher:** `notify` with debounce; ignores `target/`, `node_modules/`, `.git/`; emits batched change events.
- **Terminals:** `portable-pty` for the user terminal (ConPTY on Windows). Agent terminals are plain piped processes with output ring buffers (bounded), mirrored read-only to the UI.
- **Settings and state:** JSON in the app data dir (endpoint, model, ignore globs, window/layout, recent folders), written atomically.
- **Model endpoint probe:** `GET /v1/models` health check, auto-detect common ports (Ollama 11434, LM Studio 1234, llama-server 8080); GPU memory via `nvidia-smi --query-gpu` polling (low rate, only while visible).
- **Logging:** structured file logs with rotation; a "copy diagnostics" action.

### 7.2 IPC

Tauri commands for request/response; Tauri `Channel`s for streams (agent updates, terminal output, watcher events). Streams are coalesced in the host (~16 ms batches) and capped, so the webview receives bounded, batched messages. The frontend only sees typed, narrow commands (`open_folder`, `read_file`, `save_file`, `agent_prompt`, `agent_cancel`, `resolve_permission`, ...). Tauri capabilities are minimal and CSP is strict (no remote content).

### 7.3 Frontend

React + TypeScript + Vite; Zustand for state. Monaco used directly (not a wrapper) for control of models, view state and the diff editor; workers wired through Vite `?worker` imports with `MonacoEnvironment.getWorker`; only needed language workers loaded. xterm.js for terminals.

Panels: activity bar, virtualised file tree, editor tabs (one Monaco model per file, view state preserved), bottom panel (terminal, agent terminals, problems later), right agent panel, status bar.

Agent panel: mode switch (Plan/Build), stop button, streamed markdown, tool calls as collapsible cards (name, args, status, truncated result), permission cards (edit = diff view; command = exact command line), turn state (idle/running/waiting-for-approval), error banners with retry.

### 7.4 UI/UX (approved direction: "Workbench v2" mockup)

Reference mockup: `.superpowers/brainstorm/14279-1795017487/content/workbench.html` (live theme switcher).

- **Layout:** title bar (project, command bar `Ctrl+K`, model chip with health dot) / slim activity rail / file panel with a "Saved results" list (build logs, screenshots, UI trees stored as files) / editor with tabs / bottom panel split in two (Terminal-Build on the left, Running app on the right) / agent panel on the right / thin status bar.
- **Agent panel:** Plan/Build switch; two budget gauges at the top (context: system, chat, tool results; GPU memory: model, cache, free); a step list (read, build, edit, rebuild, run) with status marks; an approval card with the diff and Accept/Reject; composer at the bottom.
- **Edits:** the same diff is shown inline in the editor next to the changed lines, with Accept/Reject there too.
- **Build panel:** shows the one-line summary the model sees plus a link to open the full saved log.
- **Running app panel (used from project 3):** live window view, "agent is operating <app>" banner with a Stop button, status-bar hint for the emergency stop `Ctrl+Alt+Esc`.
- **Themes:** six built in as design tokens (Harbor, Graphite, Aurora, Paper light, Phosphor, Ember). Default dark = Harbor, default light = Paper (my call); the rest are user-selectable. All colours come from CSS variables so a theme is data, not code.
- **Type:** Instrument Sans for UI, Spline Sans Mono for code and numbers; editor font and size user-configurable. Fonts are bundled with the app (no network fetch).
- **Quality floor:** keyboard reachable everywhere with visible focus, WCAG AA contrast checked per theme, respects reduced-motion, text scales with an app zoom setting, layout panels resizable and remembered.

### 7.5 Required changes to agent-core

The shell needs real structure instead of text markers. Work items for this project:
1. Emit ACP `tool_call` / `tool_call_update` notifications (id, title, status, truncated output) instead of `[tool: x]` text chunks; keep text chunks for model prose.
2. Make `session/cancel` real: abort the in-flight HTTP stream and any wait, return stop reason `cancelled`.
3. Emit usage updates (prompt/completion tokens, tokens/s) per model call.
4. Persist and reload session history (host-owned file or a `session/load`) for crash recovery.
5. Keep the existing smoke test passing; extend it for the new events.

## 8. Efficiency rules

Idle target under 300 MB RAM for shell+host (excluding the model); virtualised tree and chat lists; no polling while hidden; Monaco languages loaded on demand; output ring buffers; diffs over a size cap shown as "large change" with summary instead of rendering; file size cap for opening in Monaco (with read-only "open anyway").

## 9. Failure handling

| Failure | Behaviour |
|---|---|
| Agent process dies | Banner, auto-restart with backoff, session restored, pending permission cleared |
| Model server unreachable or slow | Clear status in status bar and chat; agent-core already times out (token cap, read timeout, deadline, stuck-output abort) |
| Permission request ignored | Stays pending visibly; turn can be cancelled |
| File changed on disk | Conflict prompt; never overwrite silently |
| IPC flood | Host coalescing and caps |
| Crash of the IDE | Workspace state and unsaved-buffer recovery files restored on next start |

## 10. Testing

- `ide-host`: unit tests (path sandbox incl. symlink/junction escape, ACP framing, conflict guard, ring buffers, supervisor restart) and integration tests against a mock agent binary (extends the existing mock-llama/fake-editor idea).
- Frontend: Vitest for stores and reducers; component tests for permission/diff cards.
- End to end: scripted run against `agent-core` + a mock model server (extends `scripts/smoke.py`); manual checklist on a real model (qwen-4b) for each release; Tauri WebDriver UI automation evaluated but not required for v1.
- The existing 3-mode smoke test and live suite must keep passing.

## 11. Definition of done for v1

Open a C++/Rust/TS folder, edit and save with Monaco; run Plan and Build turns against a real local model; approve a diff edit and see it land undoably; reject an edit; approve and reject a command; stop a running turn; kill the agent process and see recovery; stop the model server and see a clear error; all tests above green; installer builds and runs on a clean Windows 10 machine.

## 12. Left to the builder

Exact visual design and theming, keybindings, icon set, file-tree and tab micro-behaviour, state-store layout, settings UI, installer packaging details, crate versions.

## 13. Calls I made (not discussed with the user)

- React + TypeScript + Vite + Zustand for the frontend.
- Spawn the agent from Rust (`tokio::process`) rather than the Tauri shell plugin, to keep the webview free of process-spawn permission.
- Split logic into a Tauri-free `crates/ide-host` crate for testability.
- `portable-pty` + xterm.js for the terminal; `notify` for the watcher.
- Agent edits applied as Monaco undoable edits when the file is open.
- Windows-only testing in v1.
- Unverified: whether Unsloth Studio exposes an OpenAI-compatible endpoint; crate choices were not checked against current versions.
