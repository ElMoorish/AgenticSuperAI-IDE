# ASA AgenticSuperAI IDE: project plan

A fully local, agentic coding IDE for ordinary hardware (target: Windows 10, 8 GB VRAM GPU, 16 GB RAM).
This document is the single place to read what exists, what was measured, what is still unknown, and
what we intend to build and publish. Every number below was measured on one machine (see "Test machine");
anything not measured is marked **estimate** or **unverified**.

## 1. Problem and goal

Strong open models keep growing, but most developers have a mid-range GPU. Running a 27B+ model on 8 GB of
VRAM is possible only by moving weights between disk, RAM and GPU, and the speed is then set by bytes read per
token divided by storage bandwidth, not by compute. We want a coding agent that stays usable on such hardware by
(a) choosing the right inference backend for the model and machine, and (b) keeping the agent's context small
and safe.

**Goal:** an open-source IDE whose agent runs entirely on the user's machine, with a backend manager that picks,
launches and honestly benchmarks local model servers.

**Non-goals (for now):** cloud inference, training, matching frontier-model quality.

## 2. What exists today (shipped in this repository)

| Part | State | Where |
|---|---|---|
| IDE shell: Tauri v2 + React, Monaco editor, xterm terminals, settings, command palette, crash recovery, diagnostics | Done, merged to `main`. Tests: Vitest 143 passing; Cargo workspace passing | `apps/shell`, `crates/ide-host` |
| Agent sidecar (Rust): tool loop (list/find/search/read/edit/compile/shell), plan and build modes, text (XML) and native tool calls, loop guard, edit approval flow | Done | `crates/agent-core` |
| Any OpenAI-compatible backend (llama.cpp, Unsloth Studio, others) via one address and optional API key | Done | Settings dialog |
| AirLLM server: OpenAI-compatible wrapper with a cleanup patch and resident-layer option | Done, tested on a 4B model only | `tools/airllm-server`, `config/start-airllm.ps1` |
| llama.cpp launch and benchmark scripts | Done | `config/`, `scripts/bench-llama-moe.ps1` |

Not verified: the shell running in a real window against a real model for long sessions (checklist in
`docs/shell-checklist.md`, section 6); the installer (NSIS bundling fails with os error 32 on the test machine).

## 3. Measurements so far

### Test machine
Windows 10 Pro, RTX 4060 (8 GB), 15.9 GB RAM (about 5 GB free at test time), E: = SK hynix NVMe SSD.
llama.cpp b12359 (CUDA 12.4). AirLLM 4.0.0, PyTorch 2.6.0+cu124, transformers 5.23, bitsandbytes 0.50.2.

### 3.1 Why layer streaming is slow: where the time goes (AirLLM, Qwen3.5-4B, 4-bit)

| Stage | Result |
|---|---|
| Stock BF16 | 12.0 s/token |
| 4-bit (files compressed on disk) | 8.5 s/token |
| Per-layer `gc.collect()` + `empty_cache()` (stock `clean_memory`) | 66% of the time |
| After replacing it with `empty_cache()` only | 1.6 s/token (4.8x faster) |
| After that fix, layer reads | about 80% of the remaining time; about 1.3 GB/s effective |
| Sequential read of E: (mostly cold) | 1.5 GB/s |
| Pinned RAM to GPU copy (PCIe) | 11.7 GB/s |

### 3.2 Keeping layers resident on the GPU (`--vram-gb`)

AirLLM's 4-bit mode shrinks files on disk only; on the GPU a layer is expanded to 16-bit, so the budget is counted
at the expanded size.

| Resident VRAM | Layers kept (of 33) | Speed |
|---|---|---|
| 0 GB | 0 | 1.65 s/token |
| 2 GB | 10 | 1.1 s/token |
| 4 GB | 20 | 0.66 s/token |

### 3.3 MoE expert offload in llama.cpp (OLMoE-1B-7B Q4_K_M, 3.92 GiB, 1.7B active parameters)

| Configuration | Prompt (pp128) tok/s | Generation (tg32) tok/s |
|---|---|---|
| All on GPU | 3684 (high variance, +/- 1783) | 247 |
| Experts on CPU, attention on GPU (`-ncmoe 99`) | 214 | 36 |
| Half the expert layers on CPU (`-ncmoe 8`) | 399 | 63 |
| CPU only (`-ngl 0`, CUDA build) | 204 | 29 |

Caveats: the model fits in RAM, so this measures offload behaviour, **not SSD streaming**; it is a 2024 model, so
nothing here says anything about coding quality; 3 repeats per cell.

### 3.4 MoE expert offload beyond RAM (Qwen3.5-35B-A3B, UD-IQ4_XS, 16.28 GiB, 3B active parameters)

The file is larger than the free RAM (about 5 GB), so experts are paged from the SSD. `llama-bench`, 2 repeats.

| Configuration | Prompt (pp128) tok/s | Generation (tg32) tok/s |
|---|---|---|
| All experts on CPU/SSD (`-ncmoe 99`) | 12.0 +/- 1.2 | 8.95 +/- 2.40 |
| 32 expert layers on CPU (`-ncmoe 32`) | 20.9 +/- 9.9 | 16.7 +/- 0.4 |
| 26 expert layers on CPU (`-ncmoe 26`) | 28.1 +/- 14.6 | 15.5 +/- 1.8 |

Caveats: 2 repeats and large spread (page-cache state differed between runs); prompt processing is slow, which
matters for an agent whose system prompt is about 1,000 tokens; no quality or coding evaluation was done.
For scale, the dense 4B through AirLLM (3.2) ran at 0.6 to 1.6 s per token, but the models differ in size and quality.

### 3.5 Vision check (same model, `--mmproj mmproj-F16.gguf`, llama-server, `-ncmoe 32`)
`scripts/vision-check.py` sends a generated image of three colored blocks. The model answered "Red, Green, Blue"
(correct order) in 1.7 s. With thinking enabled (the server default for this model), 60 `max_tokens` were
consumed by hidden reasoning and the visible answer was empty, so clients must either allow enough tokens or send
`chat_template_kwargs: {"enable_thinking": false}`. One synthetic image only; this is a smoke test, not an evaluation.
The IDE itself cannot send images yet (no image content blocks in the agent protocol or the chat UI).

### 3.6 End-to-end agent check
The agent (plan mode) ran against the AirLLM server with the 4B model: fell back to text tool calls, listed the
folder, read the README and answered correctly (3 model calls, 110 s). One easy task, one model.

Same check with the Qwen3.5-35B-A3B (UD-IQ4_XS) on llama-server (`-ncmoe 32`, `-c 16384`, `--jinja`), using the
IDE agent's native tool calls (no fallback needed):

| Task | Mode | Result | Time |
|---|---|---|---|
| "What does this project do?" | plan | `list_dir`, `read_file`, `list_dir src`, correct answer | 110 s |
| "Add a `--lines` option to `src/main.py`" | build | `list_dir`, `read_file`, one `edit_file_search_replace`; edited program prints words 6 / lines 3 on a test file | 161 s |

Server log during these runs: generation about 11 to 16 tok/s, prompt processing about 18 to 44 tok/s.
Limits: the test driver (a script standing in for the editor, not the IDE window) does not implement terminals, so
the agent's compile and syntax checks failed with "editor does not handle terminal/create"; the agent recovered by
re-reading the file. Two small tasks on one tiny project, one run each; no repeat spread, no harder tasks, and the
edit approval UI was not exercised.

**In the real IDE window** (release build, WebView2 driven through its remote-debugging port, same 35B server, a throwaway
project; the user's settings were backed up and restored afterwards):

| Step | Result |
|---|---|
| Settings: endpoint to the local server, no API key | "Connected. 1 model available."; the 35B appeared in the model picker; the title-bar chip updated |
| Plan mode: "What does this project do?" | Correct answer; `list_dir`, `read_file`, `list_dir`, `read_file` shown with check marks; context 278/16k |
| Build mode: add a `--lines` option | Edit approval card with the diff and Accept/Reject/side-by-side; after approval the file changed, the editor opened it, a `py_compile` check ran in an "Agent #1" terminal tab, and the agent's summary was correct; the edited program prints words 6 / lines 3 |

Observations, fixed afterwards (unit-tested, then re-checked in the real window with the 35B: bold and `code` rendered, the
read_file step appears before the answer, status bar read 9.2 tok/s): the answer text showed raw `**bold**`
(a small safe Markdown renderer now handles code, headings, lists, bold, italic); tool steps were listed after the answer text
(the chat is now one timeline in arrival order); the status-bar "Speed" read 1.4 to 3.9 tok/s while llama-server logged 11 to
16 tok/s (it averaged over the whole turn including prompt processing; the agent now times from the first generated token).
Fixed afterwards: the IDE log (`asa.log`) now records turns, approval requests (edit path and size, or the command, never file
contents or prompt text), accept/reject, and whether an edit was applied; covered by host tests. The approval was resolved before the test driver's click, most likely by the
user (the window was on their screen); no auto-accept path was found in the code.

### 3.6b Dense 27B through llama.cpp (Qwen3.8-27B coder fine-tune, Q4_K_M MTP file, 18.5 GB, `-ngl 20` of 64 layers)

One cold run unless noted; the 120B download was running in the background during the MTP runs, which may have slowed them.

| Test | Prompt tok/s | Generation tok/s |
|---|---|---|
| `llama-bench` pp128 / tg32 (cold) | 3.97 | 1.43 |
| server, plain decoding, run 1 / run 2 (warm) | 1.48 (34 tokens) | 0.78 / 1.46 |
| server, `--spec-type draft-mtp`, run 1 / run 2 | 1.32 | 0.36 / 0.29 (42 of 48 drafted tokens accepted) |

Reading: a dense 27B on this machine generates about 1.4 tok/s (a few seconds for a short answer, minutes for a long one) and
processes prompts at about 4 tok/s, so a first agent request of about 900 tokens would take roughly four minutes. MTP drafting
accepted 87% of its guesses but was about four times SLOWER here, so use the plain path (the MTP file also runs without it).

**Through the IDE (real window, plain decoding, 120B download running at the same time):** the task "change one word in
README.md, read the file first" finished correctly in 464 s (7 min 44 s): read_file, then one edit that I accepted, README
changed as asked. First request: 877 prompt tokens in 105 s (8.3 tok/s). Generation 0.57, 0.56 and later 0.15 tok/s; the
slow last request overlapped heavy disk use by the download, so treat it as a worst case. Usable for patient, small edits;
not for interactive back-and-forth.
Not measured: other `-ngl` values, a non-MTP file, MTP without the background download.

Bottleneck status for the 27B: HYPOTHESIS ONLY, not profiled. The file (18.5 GB) is larger than free RAM (about 5 GB), so most weights are
read from the NVMe drive for every token; 1.4 tok/s x ~17 GB is about 24 GB/s of weight reads per second only if all were streamed, which is
far above the drive, so the real split between GPU layers, RAM cache and disk is unknown. To check later: disk read rate and GPU use during
generation, `-ngl` sweep, KV cache size, `--no-mmap`, and a run with nothing else using the disk. The 27B files were deleted after these
numbers were recorded, so any re-test needs a re-download (about 19 GB).

### 3.7 Working conclusions (and their limits)
1. Decode speed of any streaming approach is bounded by (bytes read per token) / (storage bandwidth). Dense 27B on this machine is
   expected at several seconds per token however well tuned (**estimate**, from 3.1 and 3.2, not measured on a 27B).
2. The way past the bound is to read fewer bytes per token: mixture-of-experts models with expert-aware offload.
   Measured on a small model that fits in RAM (3.3) and on a 35B-A3B larger than RAM (3.4): 9 to 17 tok/s generation
   with llama.cpp offload, versus about 1 to 2 tok/s for a dense 4B through AirLLM (different models).
3. Keeping part of the model resident on the GPU gives a gain proportional to the share kept (3.2).

## 4. Roadmap

Each phase needs its own short design before code (the repository convention).

| Phase | Deliverable | Done when |
|---|---|---|
| P1 | **Real-window verification** of the shell with a real model | `docs/shell-checklist.md` section 6 fully ticked |
| P2 | **MoE beyond RAM**: llama.cpp expert offload measured on a 35B-A3B (3.4); remaining: cold-cache protocol, more repeats, a coding-quality check, and an SSD-streaming fork if stable | Table 3.4 with a cold-cache procedure and a quality check |
| P2b | **Vision in the IDE** (built): paste, drop or pick up to 4 pictures (downscaled to 1568 px), ACP `image` blocks, `image_url` parts to the server (llama-server needs `--mmproj`); pictures are sent on every step of the turn and kept in history only as a note. Verified in the real window with the 35B + mmproj: a pasted red/green/blue picture was answered "Red, Green, Blue". Not yet: a screen-capture button, a "this model has no vision" hint from the server's `/props` (it reports `modalities.vision`), the "server rejected the image" message was not exercised | Ask a question about a pasted screenshot in the IDE and get a correct answer (done); polish items above |
| P2c | **Approval popup** (built): a small always-on-top window (second Tauri window, `overlay.html`) that appears only while the agent waits for an answer and the IDE window is not the one in use (another app in front, or the IDE minimized). It sits at the bottom-right of the screen (work area, above the taskbar) until the user drags it; the new spot is remembered and ignored if that screen is gone. It never takes the keyboard focus. One request at a time with Accept/Reject (edits) or Allow/Deny (commands), "+N waiting", "Open IDE"; no approve-all. Answers go through the IDE window's normal path. Verified in the real app with a scripted model: shown over another application, hidden when the IDE is focused, shown again when it is minimized, remembered position, edit applied after a click on the popup; 13 component tests
| P3 | **Build/run with context-safe digests**: compile C++ and run apps, return short summaries instead of raw logs | Agent fixes a failing build in a sample repo without exceeding a set context budget |
| P4 | **Model manager**: detect VRAM, RAM and disk speed; recommend backend and quantization; download to a chosen drive; start and stop servers; show a measured speed estimate before large downloads | Fresh machine: pick a model, get a working endpoint, estimate within a stated error of the measured speed |
| P5 | **AirLLM improvements** worth upstreaming: cleanup patch (4.8x), resident layers, keep-4-bit-on-GPU (untested), prefetch with compression | Reproducible benchmark and a pull request or a documented fork |
| P6 | **Desktop control** of any application with context-safe output | Separate design |
| P8 | **70 GB model test** (planned): a model of about 70 GB on this machine (16 GB RAM, 8 GB VRAM). A dense 70B streams about 70 GB per token (minutes per token at SSD speed), so the realistic candidates are MoE models with expert offload; measure prompt and generation speed cold and warm, first-token delay, disk use, and agent tool-call quality, following the protocol of 3.4 | Table like 3.4 for the 70 GB model, with the cold-cache procedure and a quality check |
| P7 | Installer and release | NSIS bundle builds; signed or documented SmartScreen note |

Open questions: does AirLLM expert streaming work for Qwen3.5-MoE (it needs a per-model setting that was not found)?
Can weights stay 4-bit on the GPU in AirLLM? Which SSD-streaming llama.cpp forks are stable enough to depend on?

## 5. Publishing plan

### 5.1 GitHub launch checklist
- [ ] Choose a license and add `LICENSE` (decision for the owner).
- [ ] README: one-paragraph pitch, a screenshot or short clip (needs P1), quick start, hardware requirements, honest limits.
- [ ] `docs/`: this plan, the shell checklist, a `BENCHMARKS.md` with scripts and raw outputs for every table above.
- [ ] Reproducibility: pin versions (Section 3 test machine), one command per table, state cold or warm cache.
- [ ] CI: Cargo tests and Vitest on push (flaky terminal tests listed as known issue first).
- [ ] Issue templates; a "known gaps" list copied from the checklist.
- [ ] Release: a tagged build only after P1 and P7.

### 5.2 arXiv paper: candidate framing
Working title: *Local agentic coding on 8 GB GPUs: where streaming inference spends its time, and what to do about it.*

Candidate contributions, each only if the evidence exists when we write:
1. A per-stage profile of layer-streaming inference showing the bytes-per-token / bandwidth bound (3.1). **Evidence exists** (one 4B model, one machine).
2. A bug-level finding: a per-layer garbage-collection call dominating AirLLM's 4-bit runtime, and the fix (4.8x). **Evidence exists**; needs a second model and machine to avoid "one setup" criticism.
3. Resident-layer budgeting counted at expanded size, with measured scaling (3.2). **Evidence exists** (4B).
4. A comparison of dense layer streaming and MoE expert offload on the same hardware. **Not yet**: needs P2 with models larger than RAM.
5. A backend-selection method for the model manager. **Not yet**: needs P4.

Method requirements before submission: at least two models per backend, cold-cache protocol, 5+ repeats with spread,
fixed prompts, quality check on a small coding benchmark (otherwise speed numbers say nothing about usefulness),
and a related-work section covering AirLLM, llama.cpp expert offload and the SSD-streaming forks, with citations
checked by hand (several fork READMEs we saw are self-reported or AI-written; do not cite their numbers as fact).

Suggested outline: Introduction; Background (layer streaming, MoE offload); Setup; Profiling results; Optimizations;
MoE offload comparison; Agent integration and a small end-to-end task study; Limitations; Related work; Conclusion.

## 6. Risks

| Risk | Mitigation |
|---|---|
| Single-machine results | Repeat on a second GPU/disk before any claim in a paper |
| Speed claims without quality checks | Add a small coding benchmark to every comparison |
| Dependence on experimental forks | Treat them as optional backends; keep stock llama.cpp as the default |
| Small local models fumble tool calls (seen with 9B and 4B) | Model manager recommends a coder-tuned model; keep text tool fallback and loop guard |
| Windows-specific issues (installer, antivirus locks) | Document; test the installer on a clean VM |
