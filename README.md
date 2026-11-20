# ASA Agentic Super AI IDE

MIT licensed. A local-first agentic IDE: the model runs on your machine, nothing is sent to a cloud.

Local-first agentic IDE for an RTX 4060 (8 GB) / 16 GB RAM / Windows machine.
Design: `docs/Merged Local Agentic IDE Architecture.docx` (merges the two earlier docs and the Betelgeuse video).

## Layout

| Path | What |
|---|---|
| `crates/agent-core` | Rust agent. ACP-style JSON-RPC over stdio. Modules: `agent` (Plan/Build modes), `repair` (tool-call repair), `loopguard` (loop breaker), `budget` (VRAM pre-flight), `provider` (llama-server client) |
| `crates/ide-host` | Tauri-free host library: workspace sandbox, atomic file writes, agent supervisor, terminals, watcher, settings, recovery, log |
| `apps/shell` | The IDE: Tauri v2 + React + Monaco + xterm. `src-tauri` is the thin command layer, `src` the UI |
| `extension/` | Thin VS Code extension that spawns the agent (Phase 1 shell; Tauri shell comes later) |
| `config/` | Runtime launch scripts (llama-server flags for 8 GB) |
| `docs/` | Source architecture documents |

## Run

```bash
cargo test                      # unit tests
cargo build                     # builds target/debug/agent-core
cd extension && npm install && npm run compile
```

Start a model (`config/start-llama-server.ps1 -Model <gguf>`), open `extension/` in VS Code, press F5, and run
"ASA: Ask Agent" from the command palette. Without a running model the agent replies `[no model] ...`.

## Status (Phase 1 scaffold)

Done: SSE streaming, tool loop (max 8 steps) with repair and loop guard, `read_file` and `edit_file_search_replace`
executed through the editor (fs + permission prompt), SEARCH/REPLACE engine (`diff.rs`), mode gating, VRAM budget math.
Test: `cargo test` and `python scripts/smoke.py` (mock model + fake editor, no GPU needed).
Also done: `run_shell_scoped` and `run_compiler_check` (cargo / tsc / cmake / luacheck auto-detect) through editor terminals,
with a command policy (`shell.rs`: safe list runs unprompted, unknown asks, destructive/operators denied) and output truncation.
Also done: after each approved edit the agent re-reads the file (reports format-on-save changes) and, for source files, runs the detected compile check and appends the result (disable with `ASA_AUTO_DIAGNOSTICS=0`).
Live test with a real local model (Ollama works as-is, same OpenAI-style API): `python scripts/live.py [model] [url]`.
It builds a broken Cargo project in a temp dir and lets the agent fix it with real files and real cargo.
Tool calling: native (OpenAI `tools` + streamed `tool_calls`) by default. The XML `<tool_call>` format is the fallback: it is used
automatically if the server rejects `tools` (HTTP 400/422, e.g. a model without tool support) or if the model answers in text,
and can be forced with `ASA_TOOL_MODE=xml`. `python scripts/smoke.py` tests native, xml and fallback modes.
Live suite: `python scripts/live.py [model] [url]` runs 11 scenarios (compile errors, cross-file, failing tests, missing feature,
rename refactor, borrow checker, 250-line file, Plan-mode no-edit, prompt injection) with real cargo and objective pass/fail checks.
Env: `ONLY=a,b`, `TRIALS=n`, `VERBOSE=1`. The harness approves edits but refuses command prompts, like a careful user.
Not yet: LSP diagnostics (the check is a full project compile, not incremental), baseline-vs-new error diffing, LSP, code index, broker, expert-paging tier (spike only, see spec section 7).


## IDE shell

```bash
cd apps/shell && npm install
npm run tauri dev               # run the IDE (needs Rust and WebView2)
npm test                        # UI tests
cargo test --workspace          # host, agent and end-to-end tests (scripted model, real agent process)
cargo build -p agent-core --release && cd apps/shell && npm run tauri build   # installer (NSIS)
```

Manual verification on a real machine and model: `docs/shell-checklist.md`.


## Local models: GGUF and safetensors

The IDE talks to any OpenAI-compatible server, and its Models dialog (the model chip in the title bar, or the boxes icon in
the left bar) can find and start two kinds of models:

| | GGUF files | Hugging Face safetensors folders |
|---|---|---|
| Engine | [llama.cpp](https://github.com/ggml-org/llama.cpp) `llama-server` | [AirLLM](https://github.com/lyogavin/airllm), through `tools/airllm-server` |
| Best for | Daily use. Mixture-of-experts models can keep experts in system memory (`--n-cpu-moe`) | Trying models far larger than your GPU or RAM, at a large cost in speed |
| Suggested settings | Read from the GGUF header (layers, experts, context) and your GPU memory | Plain-words speed estimate (the model is re-read from disk for every token) |

The dialog scans folders you add, Ollama, LM Studio, the Hugging Face cache and a `models` folder on each drive. It starts the
engine, shows the log while loading and connects the IDE when the server is ready. Setup for AirLLM: `tools/airllm-server/README.md`.
Measured results and the bottleneck analysis: `docs/PROJECT-PLAN.md` (section 7).
