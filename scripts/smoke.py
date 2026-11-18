"""End-to-end smoke test (also covers structured tool events, usage and real cancel): mock llama-server (SSE) + fake editor client, real agent binary.
Run: cargo build && python scripts/smoke.py
Checks streaming, tool-call repair, read_file and edit_file_search_replace through the editor,
permission rejection, and Plan-mode edit blocking."""
import json, os, subprocess, sys, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer

# Modes: cancel_prefill = mock delays response headers ~3 s (prompt prefill); cancel must still return within 500 ms.
#        cancel = slow-streaming mock; session/cancel must end the turn with stopReason "cancelled" within 500 ms.
#        native = agent sends `tools`, mock answers with tool_calls deltas;
#        xml = ASA_TOOL_MODE=xml; fallback = mock rejects `tools` with HTTP 400 and the agent must retry in XML.
if len(sys.argv) < 2:
    for mode in ("native", "xml", "fallback", "cancel", "cancel_prefill"):
        print(f"=== mode: {mode}")
        if subprocess.run([sys.executable, "-I", os.path.abspath(__file__), mode]).returncode: sys.exit(1)
    print("ALL MODES PASSED"); sys.exit(0)
MODE = sys.argv[1]
seen = {"tools": 0, "rejected": 0, "xml_prompt_with_tools": 0, "native_prompt_with_xml": 0}

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "agent-core.exe" if os.name == "nt" else "agent-core")
tmp = tempfile.mkdtemp()
events = []  # structured session/update notifications other than message chunks
ran = []  # commands the agent actually launched in the (fake) terminal
target = os.path.join(tmp, "a.txt")
open(target, "w").write("hello\nworld\n")

DIFF = "<<<<<<< SEARCH\nworld\n=======\nthere\n>>>>>>> REPLACE"
# scripted model: choose the reply from what the last message says
def script(messages):
    last = messages[-1]["content"]
    if messages[-1]["role"] == "tool" or "<tool_result>" in last:
        if "reformatted" in last: return "Saw reformat."
        if "Diagnostics" in last and "applied" in last:
            return "Edit applied; diagnostics " + ("clean." if "clean" in last else "reported" + (" (truncated)." if "lines omitted" in last else "."))
        if "command refused" in last: return "Refused."
        if "exit code: 1" in last: return "Compiler failed" + (" (truncated)." if "lines omitted" in last else ".")
        if "rejected this command" in last: return "Command rejected."
        if "applied" in last: return "Done: replaced world with there."
        if "rejected" in last: return "Understood, not editing."
        if "not available" in last: return "Plan mode cannot edit."
        return "I read the file."
    call = lambda n, a: "<tool_call>" + json.dumps({"name": n, "arguments": a}) + "</tool_call>"
    d = lambda path, a, b: call("edit_file_search_replace", {"path": path, "diff": f"<<<<<<< SEARCH\n{a}\n=======\n{b}\n>>>>>>> REPLACE"})
    if "fixsrc" in last: return d("m.rs", "fn a() {}", "fn b() {}")
    if "edittxt" in last: return d("a.txt", "there", "world")
    if "reformat" in last: return d("f.txt", "hello", "hello2")
    if "danger" in last: return call("run_shell_scoped", {"command": "rm -rf /"})
    if "unknown" in last: return call("run_shell_scoped", {"command": "python x.py"})
    if "compile" in last: return call("run_compiler_check", {})
    if "edit" in last:
        # trailing {} on purpose: exercises repair
        return "<tool_call>" + json.dumps({"name": "edit_file_search_replace", "arguments": {"path": "a.txt", "diff": DIFF}}) + "{}</tool_call>"
    return "<tool_call>" + json.dumps({"name": "read_file", "arguments": {"path": "a.txt"}}) + "</tool_call>"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        has_tools = "tools" in body
        sysmsg = body["messages"][0]["content"]
        if has_tools:
            seen["tools"] += 1
            if "<tool_call>" in sysmsg: seen["native_prompt_with_xml"] += 1
            if MODE == "fallback":  # e.g. Ollama: model does not support tools
                seen["rejected"] += 1
                self.send_response(400); self.end_headers(); self.wfile.write(b'{"error":"does not support tools"}'); return
        elif MODE == "native" or "<tool_call>" not in sysmsg:
            seen["xml_prompt_with_tools"] += 1  # a request without tools whose prompt lacks the XML format
        if MODE == "cancel_prefill":
            time.sleep(3)  # no headers yet
            try:
                self.send_response(200); self.end_headers()
            except OSError: pass
            return
        if MODE == "cancel":
            self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
            try:
                for i in range(200):
                    self.wfile.write(("data: " + json.dumps({"choices": [{"delta": {"content": f"word{i} "}}]}) + "\n\n").encode()); self.wfile.flush()
                    time.sleep(0.1)
                self.wfile.write(b"data: [DONE]\n\n")
            except OSError: pass  # the agent hung up after cancel
            return
        text = script(body["messages"])
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        if has_tools and text.startswith("<tool_call>"):
            # native: parse our scripted call (ignoring the deliberate trailing {}) and stream it as tool_calls deltas
            obj, _ = json.JSONDecoder().raw_decode(text[len("<tool_call>"):])
            frags = [{"index": 0, "id": "c1", "function": {"name": obj["name"], "arguments": ""}}]
            args = json.dumps(obj["arguments"])
            frags += [{"index": 0, "function": {"arguments": args[i:i+9]}} for i in range(0, len(args), 9)]
            for f in frags:
                self.wfile.write(("data: " + json.dumps({"choices": [{"delta": {"tool_calls": [f]}}]}) + "\n\n").encode()); self.wfile.flush()
            self.wfile.write(b"data: [DONE]\n\n"); return
        for i in range(0, len(text), 7):  # stream in small pieces
            self.wfile.write(("data: " + json.dumps({"choices": [{"delta": {"content": text[i:i+7]}}]}) + "\n\n").encode()); self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")

srv = HTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, ASA_LLM_URL=f"http://127.0.0.1:{srv.server_port}")
if MODE == "xml": env["ASA_TOOL_MODE"] = "xml"
p = subprocess.Popen([EXE], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)

def send(o): p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()

def rpc(i, method, params, allow=True):
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    chunks, calls = [], []
    while True:
        m = json.loads(p.stdout.readline())
        if m.get("method") == "session/update":
            u = m["params"]["update"]
            if u["sessionUpdate"] == "agent_message_chunk": chunks.append(u["content"]["text"])
            else: events.append(u)
        elif "method" in m:  # request from the agent to the editor
            calls.append(m["method"]); pr = m["params"]
            if m["method"] == "fs/read_text_file":
                if not os.path.exists(pr["path"]):
                    send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": "not found"}}); continue
                res = {"content": open(pr["path"]).read()}
            elif m["method"] == "terminal/create": ran.append(pr["command"] + " " + " ".join(pr["args"])); res = {"terminalId": "t1"}
            elif m["method"] == "terminal/wait_for_exit": res = {"exitCode": 1}
            elif m["method"] == "terminal/output": res = {"output": "\n".join(f"error line {i}" for i in range(200)), "truncated": False}
            elif m["method"] == "terminal/release": res = {}
            elif m["method"] == "fs/write_text_file":
                # f.txt simulates a format-on-save that rewrites the file
                open(pr["path"], "w").write(pr["content"].upper() if pr["path"].endswith("f.txt") else pr["content"]); res = {}
            elif m["method"] == "session/request_permission": res = {"outcome": {"outcome": "selected", "optionId": "allow" if allow else "reject"}}
            send({"jsonrpc": "2.0", "id": m["id"], "result": res})
        elif m.get("id") == i:
            return m, "".join(chunks), calls

def check(cond, label):
    print(("PASS " if cond else "FAIL ") + label)
    if not cond: sys.exit(1)

rpc(1, "initialize", {})
sid = rpc(2, "session/new", {"cwd": tmp})[0]["result"]["sessionId"]
prompt = lambda t, mode: {"sessionId": sid, "mode": mode, "prompt": [{"type": "text", "text": t}]}

if MODE in ("cancel", "cancel_prefill"):
    send({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": prompt("write a lot", "plan")})
    if MODE == "cancel":
        while True:  # wait until the model is actually streaming
            m = json.loads(p.stdout.readline())
            if m.get("method") == "session/update" and m["params"]["update"]["sessionUpdate"] == "agent_message_chunk": break
    else:
        time.sleep(0.5)  # the agent is now blocked waiting for response headers
    t0 = time.time()
    send({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}})
    while True:
        m = json.loads(p.stdout.readline())
        if m.get("id") == 3: break
    dt = time.time() - t0
    check(m["result"]["stopReason"] == "cancelled", "cancel: stopReason is cancelled")
    check(dt < 0.5, f"cancel: turn ended in {dt*1000:.0f} ms (< 500)")
    p.stdin.close(); p.wait(timeout=5)
    print("ALL PASSED"); sys.exit(0)

m, text, calls = rpc(3, "session/prompt", prompt("please read it", "plan"))
check(calls == ["fs/read_text_file"] and "I read the file" in text, "plan mode reads via editor, streams text")
tc = [e for e in events if e["sessionUpdate"] == "tool_call"]
tu = [e for e in events if e["sessionUpdate"] == "tool_call_update"]
check([e["status"] for e in tc] == ["pending", "in_progress"] and len(tu) == 1 and tu[0]["status"] == "completed", "tool_call pending -> in_progress -> tool_call_update completed")
check({e["toolCallId"] for e in tc} == {tu[0]["toolCallId"]} and tc[0]["kind"] == "read" and "read_file" in tc[0]["title"], "tool events share one id; kind is read")
check("hello" in tu[0]["content"] and "[tool:" not in text and "[result:" not in text, "update carries the result; old text markers are gone")
us = [e for e in events if e["sessionUpdate"] == "usage"]
check(len(us) == 2 and all(u["promptTokens"] > 0 and u["completionTokens"] > 0 and u["tokensPerSecond"] >= 0 for u in us), "one usage update per model call")

m, text, calls = rpc(4, "session/prompt", prompt("edit it", "plan"))
check("write" not in "".join(calls) and "not available" not in text or "Plan mode cannot edit" in text, "plan mode blocks edits")
check(open(target).read() == "hello\nworld\n", "file untouched by plan mode")

m, text, calls = rpc(5, "session/prompt", prompt("edit it", "build"), allow=False)
check("session/request_permission" in calls and open(target).read() == "hello\nworld\n", "rejected edit leaves file unchanged")

m, text, calls = rpc(6, "session/prompt", prompt("edit it", "build"), allow=True)
check("fs/write_text_file" in calls and open(target).read() == "hello\nthere\n", "approved edit applied (repair handled trailing {})")
events.clear()
m, text, calls = rpc(7, "session/prompt", prompt("danger", "build"))
check([e["status"] for e in events if e["sessionUpdate"] == "tool_call_update"] == ["failed"] and any(e.get("kind") == "execute" for e in events), "refused command reports a failed execute tool call")
check("terminal/create" not in calls and "Refused" in text, "destructive command refused, never reaches terminal")

m, text, calls = rpc(8, "session/prompt", prompt("unknown command", "build"), allow=False)
check("session/request_permission" in calls and "terminal/create" not in calls and "rejected" in text, "unknown command asks, rejection blocks it")

m, text, calls = rpc(9, "session/prompt", prompt("compile", "build"))
check("terminal/create" not in calls, "compiler check without project file does not launch anything")

open(os.path.join(tmp, "Cargo.toml"), "w").write("[package]\n")
ran.clear()
m, text, calls = rpc(10, "session/prompt", prompt("compile again", "build"))
check(ran == ["cargo check --message-format short"] and "session/request_permission" not in calls, "compiler check auto-detects cargo, runs unprompted")
check("Compiler failed (truncated)" in text, "long build output was truncated before reaching the model")

m, text, calls = rpc(11, "session/prompt", prompt("compile it in plan mode", "plan"))
check("terminal/create" not in calls, "plan mode has no terminal access")
open(os.path.join(tmp, "m.rs"), "w").write("fn a() {}\n")
open(os.path.join(tmp, "f.txt"), "w").write("hello\n")

ran.clear()
m, text, calls = rpc(12, "session/prompt", prompt("fixsrc", "build"))
check(ran == ["cargo check --message-format short"] and open(os.path.join(tmp, "m.rs")).read() == "fn b() {}\n", "editing a source file auto-runs the compile check")
check("diagnostics reported (truncated)" in text, "diagnostics (truncated) are appended to the edit result")

ran.clear()
m, text, calls = rpc(13, "session/prompt", prompt("edittxt", "build"))
check(ran == [] and open(target).read() == "hello\nworld\n", "editing a non-source file skips diagnostics")

m, text, calls = rpc(14, "session/prompt", prompt("reformat please", "build"))
check("Saw reformat" in text and open(os.path.join(tmp, "f.txt")).read() == "HELLO2\n", "format-on-save change is reported back to the model")
if MODE == "native":
    check(seen["tools"] > 0 and seen["native_prompt_with_xml"] == 0 and seen["rejected"] == 0, "native mode: tools sent via API, no XML format in the prompt")
elif MODE == "xml":
    check(seen["tools"] == 0, "xml mode: no tools sent")
else:
    check(seen["rejected"] == 1 and seen["tools"] == 1, "fallback: tools rejected once, then the session stayed in XML mode")
p.stdin.close(); p.wait(timeout=5)
print("ALL PASSED")
