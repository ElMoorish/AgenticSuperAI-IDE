"""Smoke test for the AirLLM server: python smoke_test.py [http://127.0.0.1:8090]"""
import json
import sys
import time
import urllib.request

base = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8090"


def get(path):
    with urllib.request.urlopen(base + path, timeout=60) as r:
        return json.load(r)


def post(body, timeout=900):
    req = urllib.request.Request(base + "/v1/chat/completions", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=timeout)


print("health:", get("/health"))
print("models:", [m["id"] for m in get("/v1/models")["data"]])
msg = [{"role": "user", "content": "Write a one-line Python function that reverses a string."}]
for i in (1, 2):
    t = time.time()
    r = json.load(post({"messages": msg, "max_tokens": 16, "temperature": 0}))
    dt = time.time() - t
    n = r["usage"]["completion_tokens"]
    print(f"run {i}: {n} tokens in {dt:.1f}s ({dt / max(n, 1):.2f} s/token) finish={r['choices'][0]['finish_reason']}")
    print("   ", repr(r["choices"][0]["message"]["content"]))
print("health:", get("/health"))
t = time.time()
first = None
text = ""
with post({"messages": msg, "max_tokens": 12, "stream": True}) as resp:
    for line in resp:
        line = line.decode().strip()
        if line.startswith("data: ") and line != "data: [DONE]":
            d = json.loads(line[6:])["choices"][0]["delta"].get("content")
            if d:
                first = first or time.time() - t
                text += d
print(f"stream: first piece after {first:.1f}s, total {time.time() - t:.1f}s: {text!r}")
