# ASA IDE shell: manual verification checklist

Run this on a real Windows 10 machine with a real local model (for example qwen 4B in Ollama, LM Studio
or llama-server). Automated tests (`cargo test --workspace`, `npm test` in `apps/shell`) cover the logic;
this list covers what they cannot: the real window, the real WebView2, a real model.

Mark each line PASS / FAIL / NOT RUN and note the date. Anything NOT RUN must be reported, not skipped.

## 0. Install

- [ ] Run the installer from `target/release/bundle/nsis/`. It installs without admin rights errors.
- [ ] `agent-core.exe` sits next to `asa-ide-shell.exe` in the install folder.
- [ ] First start on a machine without WebView2: the bootstrapper offers to install it. (Needs a clean machine. NOT RUN on the dev machine.)
- [ ] The window opens with the Harbor theme, no white flash, no console window.

## 1. Window and layout

- [ ] The custom title bar drags the window; double-click maximizes and restores (once, not twice).
- [ ] Minimize, maximize and close buttons work.
- [ ] Each splitter drags, and also resizes with arrow keys; Enter collapses/restores a panel.
- [ ] Ctrl+B toggles the file panel. Zoom buttons change the whole UI and survive a restart.
- [ ] All six themes switch live and stay after restart. Text stays readable in each.

## 2. Files and editor

- [ ] Open a folder (Open Folder). The tree lists files; ignored folders (node_modules, target) are hidden.
- [ ] Open a file, type, Ctrl+S saves. The tab dot clears.
- [ ] Edit the same file in another editor: a banner offers Reload / Overwrite / Keep mine; nothing is lost.
- [ ] A file with CRLF line endings stays CRLF after saving.
- [ ] Monaco works under the strict CSP: highlighting, find (Ctrl+F), no errors in DevTools console.
- [ ] Close a tab with unsaved changes: a dialog with Cancel as the default appears.

## 3. Agent (needs a running model)

- [ ] The model chip is green when the server runs, red "offline" when stopped, and turns green again within ~15 s of restarting it.
- [ ] Settings: wrong URL shows an error and disables Save; a detected server can be picked; the model list fills from the server.
- [ ] Ask (Plan mode) a question about the open project. The answer streams; tokens/s and context fill in the status bar.
- [ ] Build mode: ask for a small edit. ONE approval card appears with the real diff. Accept: the editor buffer changes (Ctrl+Z undoes it in one step), the file is saved, the agent continues.
- [ ] Reject: the file is unchanged and the agent says it was rejected.
- [ ] Stop (button and Ctrl+Alt+Esc) ends the turn quickly.
- [ ] A command the agent wants to run shows the exact command line; Deny works; Allow runs it in an agent terminal tab (read-only).
- [ ] The GPU gauge shows used / total VRAM.

## 4. Terminals

- [ ] Terminal 1 opens PowerShell, accepts typing, arrow keys, Ctrl+C. Resizing the panel resizes the shell (run `$Host.UI.RawUI.WindowSize`).
- [ ] `+` opens more terminals (up to 8); closing a tab ends its process.
- [ ] Output of a long command (`dir -r C:\Windows\System32`) does not freeze the UI.

## 5. Robustness

- [ ] Type in a file without saving, then kill the app in Task Manager. On restart a banner offers to restore the text; Restore brings it back as unsaved; Discard removes it.
- [ ] Kill `agent-core.exe` in the middle of a turn: the panel shows the agent restarted, and the next prompt works and remembers the earlier conversation.
- [ ] Stop the model server mid-turn: a clear error appears (no hang). Start it again: the next turn works.
- [ ] Settings > Copy diagnostics: paste it somewhere. It contains no user folder path, no file contents, no credentials.
- [ ] Logs exist under `%APPDATA%\dev.asa.ide\logs\` and stay under 5 files of 2 MB.

## 6. Not verified by automation (known gaps)

- Real WebView2 behaviour: native drag region, IPC from a real webview, Monaco workers under the CSP.
- A real model's tool-calling quality (tests use a scripted model).
- Clean second Windows 10 machine (WebView2 bootstrapper, installer).
- The non-NTFS create-new fallback.
- Fonts: none are bundled; the UI uses system font stacks.
