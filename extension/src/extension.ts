import * as vscode from 'vscode';
import * as path from 'path';
import { spawn, ChildProcess, ChildProcessWithoutNullStreams } from 'child_process';

// Thin editor client: spawns the Rust agent and speaks line-delimited JSON-RPC (ACP-style).
// The editor keeps ownership of files/terminals; permission prompts will go through
// session/request_permission once the agent starts requesting tools.

let proc: ChildProcessWithoutNullStreams | undefined;
let nextId = 1;
const pending = new Map<number, (r: any) => void>();
let out: vscode.OutputChannel;
let sessionId: string | undefined;

function agentPath(): string {
  const cfg = vscode.workspace.getConfiguration('asa').get<string>('agentPath');
  if (cfg) { return cfg; }
  // The binary lives in the IDE repo (extension/..), not in whatever project the user has open.
  return path.join(extensionRoot, '..', 'target', 'debug', process.platform === 'win32' ? 'agent-core.exe' : 'agent-core');
}
let extensionRoot = '';

function rpc(method: string, params: unknown): Promise<any> {
  const id = nextId++;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    proc!.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}


// ---- terminals: captured child processes (the VS Code terminal API cannot return output) ----
interface Term { proc: ChildProcess; output: string; truncated: boolean; limit: number; exit: Promise<{ exitCode: number | null; signal: string | null }>; }
const terms = new Map<string, Term>();
let nextTerm = 1;
const COMMAND_TIMEOUT_MS = 120_000;

function createTerminal(p: any): string {
  const q = (a: string) => (/\s/.test(a) ? `"${a}"` : a);
  const win = process.platform === 'win32';
  // shell:true on Windows so npm/npx (.cmd) resolve; the agent already rejected shell operators
  const proc = spawn(p.command, win ? (p.args ?? []).map(q) : p.args ?? [], { cwd: p.cwd || undefined, shell: win });
  const t: Term = { proc, output: '', truncated: false, limit: p.outputByteLimit ?? 65536, exit: undefined as any };
  const collect = (d: Buffer) => {
    t.output += d.toString();
    if (t.output.length > t.limit) { t.output = t.output.slice(-t.limit); t.truncated = true; }
  };
  proc.stdout?.on('data', collect);
  proc.stderr?.on('data', collect);
  t.exit = new Promise((resolve) => {
    const timer = setTimeout(() => { t.output += '\n[killed: timeout]'; proc.kill(); }, COMMAND_TIMEOUT_MS);
    proc.on('error', (e) => { t.output += `\n[spawn error: ${e.message}]`; clearTimeout(timer); resolve({ exitCode: 127, signal: null }); });
    proc.on('close', (code, signal) => { clearTimeout(timer); resolve({ exitCode: code, signal }); });
  });
  const id = `t${nextTerm++}`;
  terms.set(id, t);
  return id;
}

function reply(id: number, result?: unknown, error?: string) {
  const m = error ? { jsonrpc: '2.0', id, error: { code: -32000, message: error } } : { jsonrpc: '2.0', id, result };
  proc?.stdin.write(JSON.stringify(m) + '\n');
}

// The editor owns files and approvals; the agent only asks.
async function handleAgentRequest(msg: any) {
  try {
    const p = msg.params;
    if (msg.method === 'fs/read_text_file') {
      const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(p.path)); // includes unsaved edits
      reply(msg.id, { content: doc.getText() });
    } else if (msg.method === 'fs/write_text_file') {
      const uri = vscode.Uri.file(p.path);
      const edit = new vscode.WorkspaceEdit();
      try {
        const doc = await vscode.workspace.openTextDocument(uri);
        edit.replace(uri, new vscode.Range(doc.positionAt(0), doc.positionAt(doc.getText().length)), p.content);
        await vscode.workspace.applyEdit(edit);
        await doc.save();
      } catch {
        await vscode.workspace.fs.writeFile(uri, Buffer.from(p.content, 'utf8')); // new file
      }
      reply(msg.id, {});
    } else if (msg.method === 'session/request_permission') {
      const pick = await vscode.window.showWarningMessage(p.toolCall.title, { modal: true, detail: p.toolCall.content }, 'Allow edit');
      reply(msg.id, { outcome: { outcome: 'selected', optionId: pick ? 'allow' : 'reject' } });
    } else if (msg.method === 'terminal/create') {
      reply(msg.id, { terminalId: createTerminal(p) });
    } else if (msg.method === 'terminal/wait_for_exit') {
      reply(msg.id, await terms.get(p.terminalId)!.exit);
    } else if (msg.method === 'terminal/output') {
      const t = terms.get(p.terminalId)!;
      reply(msg.id, { output: t.output, truncated: t.truncated });
    } else if (msg.method === 'terminal/kill') {
      terms.get(p.terminalId)?.proc.kill();
      reply(msg.id, {});
    } else if (msg.method === 'terminal/release') {
      terms.get(p.terminalId)?.proc.kill();
      terms.delete(p.terminalId);
      reply(msg.id, {});
    } else {
      reply(msg.id, undefined, `unsupported method ${msg.method}`);
    }
  } catch (e) {
    reply(msg.id, undefined, String(e));
  }
}

async function ensureAgent(): Promise<void> {
  if (proc && sessionId) { return; }
  proc = spawn(agentPath(), [], { stdio: 'pipe' });
  let buf = '';
  proc.stdout.on('data', (d) => {
    buf += d.toString();
    let i: number;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      if (!line.trim()) { continue; }
      const msg = JSON.parse(line);
      if (msg.method && msg.id !== undefined) {
        void handleAgentRequest(msg);
      } else if (msg.id !== undefined && pending.has(msg.id)) {
        pending.get(msg.id)!(msg.result ?? msg.error); pending.delete(msg.id);
      } else if (msg.method === 'session/update') {
        const u = msg.params.update;
        if (u.sessionUpdate === 'agent_message_chunk') { out.append(u.content.text); }
      }
    }
  });
  proc.stderr.on('data', (d) => out.append(`[agent stderr] ${d}`));
  proc.on('exit', () => { proc = undefined; sessionId = undefined; });
  await rpc('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true }, terminal: true } });
  sessionId = (await rpc('session/new', { cwd: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? '' })).sessionId;
}

async function ask(mode: 'plan' | 'build') {
  const text = await vscode.window.showInputBox({ prompt: `Ask the agent (${mode} mode)` });
  if (!text) { return; }
  try {
    await ensureAgent();
    out.show(true);
    out.appendLine(`\n> [${mode}] ${text}`);
    await rpc('session/prompt', { sessionId, mode, prompt: [{ type: 'text', text }] });
    out.appendLine('');
  } catch (e) {
    vscode.window.showErrorMessage(`ASA agent failed: ${e}. Build it first: cargo build`);
  }
}

export function activate(ctx: vscode.ExtensionContext) {
  extensionRoot = ctx.extensionPath;
  out = vscode.window.createOutputChannel('ASA Agent');
  ctx.subscriptions.push(
    out,
    vscode.commands.registerCommand('asa.ask', () => ask('plan')),
    vscode.commands.registerCommand('asa.build', () => ask('build')),
  );
}

export function deactivate() { proc?.kill(); }
