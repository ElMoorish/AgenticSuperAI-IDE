"""Sends a generated test image to an OpenAI-compatible server (llama-server started with --mmproj).

Usage: python scripts/vision-check.py [base_url]    (default http://127.0.0.1:8080)
The image is three colored blocks, left to right: red, green, blue. A model that really sees the image
should name those colors in that order. No third-party packages needed.
"""
import base64, json, struct, sys, time, urllib.request, zlib

base = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080"
W, H = 192, 64
colors = [(220, 30, 30), (30, 190, 60), (30, 60, 220)]
rows = b""
for _ in range(H):
    row = b"\x00"
    for x in range(W):
        row += bytes(colors[x * 3 // W])
    rows += row


def chunk(tag, data):
    c = struct.pack(">I", len(data)) + tag + data
    return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
       + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
uri = "data:image/png;base64," + base64.b64encode(png).decode()

body = {"messages": [{"role": "user", "content": [
    {"type": "text", "text": "This image has three colored blocks. List their colors from left to right, one word each."},
    {"type": "image_url", "image_url": {"url": uri}}]}],
    "max_tokens": 400, "temperature": 0,
    "chat_template_kwargs": {"enable_thinking": False}}
req = urllib.request.Request(base + "/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"})
t = time.time()
try:
    r = json.load(urllib.request.urlopen(req, timeout=600))
except Exception as e:
    print("FAILED:", e)
    sys.exit(1)
msg = r["choices"][0]["message"]
text = msg.get("content") or ""
if not text and msg.get("reasoning_content"):
    print("(no content; reasoning only):", msg["reasoning_content"][:300])
print(f"answer ({time.time() - t:.1f}s): {text!r}")
low = text.lower()
ok = all(c in low for c in ("red", "green", "blue")) and low.index("red") < low.index("green") < low.index("blue")
print("PASS: names red, green, blue in order" if ok else "CHECK: colors missing or out of order")
