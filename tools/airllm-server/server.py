"""OpenAI-compatible server for AirLLM (layer-streaming inference on a small GPU).

Endpoints: GET /health, GET /v1/models, POST /v1/chat/completions (stream and non-stream).
One request runs at a time (AirLLM streams the whole model per generated token).
Point the IDE at http://127.0.0.1:8090 (Settings > Model server address).

Two speedups over stock AirLLM:
  * the per-layer clean_memory() (a full gc.collect + empty_cache after EVERY layer) is replaced
    by a cheap version (measured: 7.8 -> 1.6 s/token on a 4B model in 4-bit);
  * an optional RAM cache keeps recently used layer files in memory (--cache-gb).
"""
import argparse
import collections
import json
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import torch
import airllm.airllm_base as ab
from airllm import AutoModel
from transformers import StoppingCriteria, StoppingCriteriaList, TextIteratorStreamer


def log(msg):
    print(f"[airllm-server] {msg}", file=sys.stderr, flush=True)


# --- speedups ------------------------------------------------------------------------------------
def patch_cleanup():
    ab.clean_memory = lambda: torch.cuda.empty_cache()


class LayerCache:
    """Static cache of layer state dicts (CPU tensors): fills up to a byte budget, never evicts."""

    def __init__(self, budget_bytes):
        self.budget = budget_bytes
        self.items = collections.OrderedDict()  # name -> (state_dict, nbytes)
        self.used = 0
        self.hits = 0
        self.misses = 0

    def install(self, model):
        orig = model.load_layer_to_cpu
        cache = self

        def load(layer_name):
            hit = cache.items.get(layer_name)
            if hit is not None:
                cache.items.move_to_end(layer_name)
                cache.hits += 1
                return dict(hit[0])  # shallow copy: callers may add/remove keys
            cache.misses += 1
            sd = orig(layer_name)
            nbytes = sum(v.numel() * v.element_size() for v in sd.values() if torch.is_tensor(v))
            # Layers are read in the same order every token, so an LRU cache smaller than the model
            # evicts each layer just before it is needed again (0 hits). Fill once, never evict:
            # the first layers that fit stay resident and always hit.
            if cache.used + nbytes <= cache.budget:
                keep = {k: (v.cpu() if torch.is_tensor(v) else v) for k, v in sd.items()}
                cache.items[layer_name] = (keep, nbytes)
                cache.used += nbytes
            return sd

        model.load_layer_to_cpu = load


def keep_resident(model, budget_bytes):
    """Keep as many layers as fit in `budget_bytes` of VRAM loaded for good.

    Stock AirLLM loads every layer from disk and evicts it again for every generated token, so the
    per-token cost is (model bytes / disk speed). Layers kept on the GPU are read once and never again,
    which removes their share of that cost. Returns (layers kept, bytes kept).
    """
    # AirLLM's 4/8-bit modes only shrink the files on disk: a layer is expanded to the run dtype on the
    # GPU, so the budget must count the expanded size (parameters x dtype bytes), not the file size.
    elem = torch.empty(0, dtype=model.running_dtype).element_size()
    resident, used = set(), 0
    for idx in model._streamed_indices:
        size = sum(p.numel() for p in model.layers[idx].parameters()) * elem
        if size and used + size <= budget_bytes:
            resident.add(idx)
            used += size
    loaded = set()
    for idx in model._streamed_indices:
        module = model.layers[idx]
        orig_pre = list(module._forward_pre_hooks.values())
        orig_post = list(module._forward_hooks.values())
        module._forward_pre_hooks.clear()
        module._forward_hooks.clear()

        def pre(mod, args, orig_pre=orig_pre):
            i = mod._airllm_idx
            if i in resident and i in loaded:
                return None  # already on the GPU
            r = None
            for h in orig_pre:
                r = h(mod, args)
            loaded.add(i)
            return r

        def post(mod, args, output, orig_post=orig_post):
            if mod._airllm_idx in resident:
                return output  # keep it on the GPU
            for h in orig_post:
                output = h(mod, args, output)
            return output

        module.register_forward_pre_hook(pre)
        module.register_forward_hook(post)
    return len(resident), used


# --- model ---------------------------------------------------------------------------------------
class Engine:
    def __init__(self, path, name, compression, max_seq_len, cache_gb, thinking, vram_gb=0.0):
        patch_cleanup()
        kw = {"max_seq_len": max_seq_len}
        if compression:
            kw["compression"] = compression
        t = time.time()
        self.model = AutoModel.from_pretrained(path, **kw)
        self.tok = self.model.tokenizer
        self.name = name
        self.max_seq_len = max_seq_len
        self.thinking = thinking
        # Stop on every end-of-turn token: model configs often list only <|endoftext|> while a chat
        # turn ends with <|im_end|>; without it the model invents further user/assistant turns.
        ids = [self.tok.eos_token_id, getattr(self.model.config, "eos_token_id", None)]
        for tok_name in ("<|im_end|>", "<|endoftext|>", "<|eot_id|>", "<end_of_turn>"):
            i = self.tok.convert_tokens_to_ids(tok_name)
            if isinstance(i, int) and i >= 0 and i != self.tok.unk_token_id:
                ids.append(i)
        flat = []
        for i in ids:
            for j in (i if isinstance(i, (list, tuple)) else [i]):
                if j is not None and j not in flat:
                    flat.append(j)
        self.eos_ids = flat
        log(f"stop tokens: {flat}")
        self.lock = threading.Lock()
        self.cache = LayerCache(int(cache_gb * 1024**3)) if cache_gb > 0 else None
        if self.cache:
            self.cache.install(self.model)
        if vram_gb > 0:
            n, used = keep_resident(self.model, int(vram_gb * 1024**3))
            log(f"{n} layers kept on the GPU ({used / 1024**3:.2f} GB)")
        log(f"model ready in {time.time() - t:.0f}s (compression={compression}, cache={cache_gb} GB)")

    def prompt_ids(self, messages):
        msgs = []
        for m in messages:
            c = m.get("content", "")
            if isinstance(c, list):
                c = "".join(p.get("text", "") for p in c if isinstance(p, dict))
            msgs.append({"role": m.get("role", "user"), "content": c or ""})
        try:
            text = self.tok.apply_chat_template(
                msgs, tokenize=False, add_generation_prompt=True, enable_thinking=self.thinking)
        except TypeError:
            text = self.tok.apply_chat_template(msgs, tokenize=False, add_generation_prompt=True)
        return self.tok(text, return_tensors="pt", return_attention_mask=False)["input_ids"]

    def generate_stream(self, messages, max_tokens, temperature, top_p, stop_flag):
        """Yields text pieces; last yield is ('done', prompt_tokens, completion_tokens, finish)."""
        ids = self.prompt_ids(messages)
        room = self.max_seq_len - ids.shape[1]
        if room <= 0:
            raise ValueError(f"prompt is {ids.shape[1]} tokens, longer than the server context {self.max_seq_len}")
        max_new = max(1, min(max_tokens, room))
        streamer = TextIteratorStreamer(self.tok, skip_prompt=True, skip_special_tokens=True)

        produced = [0]
        prompt_len = ids.shape[1]

        class Stop(StoppingCriteria):
            def __call__(self, input_ids, scores, **kw):
                produced[0] = input_ids.shape[1] - prompt_len  # real token count
                return stop_flag.is_set()

        gen_kw = dict(
            inputs=ids.cuda(), max_new_tokens=max_new, use_cache=True, streamer=streamer,
            stopping_criteria=StoppingCriteriaList([Stop()]),
            eos_token_id=self.eos_ids, pad_token_id=self.eos_ids[0],
        )
        if temperature and temperature > 0:
            gen_kw.update(do_sample=True, temperature=float(temperature), top_p=float(top_p))
        else:
            gen_kw.update(do_sample=False)
        err = []

        def run():
            try:
                self.model.generate(**gen_kw)
            except Exception as e:  # surfaced to the client below
                err.append(e)
                streamer.end()

        th = threading.Thread(target=run, daemon=True)
        th.start()
        count = 0
        for piece in streamer:
            if piece:
                count += 1
                yield piece
        th.join()
        if err:
            raise err[0]
        total = max(produced[0], count)
        yield ("done", ids.shape[1], total, "length" if total >= max_new else "stop")


ENGINE = None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        log(fmt % args)

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _err(self, code, msg):
        self._json(code, {"error": {"message": msg, "type": "server_error"}})

    def do_GET(self):
        if self.path in ("/health", "/"):
            c = ENGINE.cache
            self._json(200, {"status": "ok", "model": ENGINE.name, "cache_hits": c.hits if c else 0,
                             "cache_misses": c.misses if c else 0,
                             "cache_layers": len(c.items) if c else 0,
                             "cache_mb": round(c.used / 1048576) if c else 0})
        elif self.path.rstrip("/") in ("/v1/models", "/models"):
            self._json(200, {"object": "list", "data": [{"id": ENGINE.name, "object": "model", "owned_by": "airllm"}]})
        else:
            self._err(404, "not found")

    def do_POST(self):
        if self.path.rstrip("/") not in ("/v1/chat/completions", "/chat/completions"):
            return self._err(404, "not found")
        try:
            n = int(self.headers.get("Content-Length", "0"))
            req = json.loads(self.rfile.read(n) or b"{}")
            messages = req["messages"]
        except Exception as e:
            return self._err(400, f"bad request: {e}")
        if req.get("tools"):
            # No native tool calling here. A 400 makes the IDE agent switch to its text (XML) tool format.
            return self._err(400, "this server does not support native tools; use text tool calls")
        stream = bool(req.get("stream"))
        max_tokens = int(req.get("max_tokens") or req.get("max_completion_tokens") or 256)
        temp = req.get("temperature", 0)
        top_p = req.get("top_p", 0.95)
        cid = "chatcmpl-" + uuid.uuid4().hex[:24]
        created = int(time.time())
        stop_flag = threading.Event()

        if not ENGINE.lock.acquire(blocking=False):
            return self._err(503, "AirLLM is busy with another request (one at a time)")
        try:
            t0 = time.time()
            gen = ENGINE.generate_stream(messages, max_tokens, temp, top_p, stop_flag)

            def chunk(delta, finish=None):
                return {"id": cid, "object": "chat.completion.chunk", "created": created, "model": ENGINE.name,
                        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

            if stream:
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()
                try:
                    self.wfile.write(("data: " + json.dumps(chunk({"role": "assistant", "content": ""})) + "\n\n").encode())
                    self.wfile.flush()
                    for piece in gen:
                        if isinstance(piece, tuple):
                            _, pt, ct, fin = piece
                            self.wfile.write(("data: " + json.dumps(chunk({}, fin)) + "\n\n").encode())
                            log(f"{ct} tokens in {time.time() - t0:.1f}s")
                        else:
                            self.wfile.write(("data: " + json.dumps(chunk({"content": piece})) + "\n\n").encode())
                        self.wfile.flush()
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError, OSError):
                    stop_flag.set()  # client went away: stop generating
                    for _ in gen:
                        pass
                self.close_connection = True
            else:
                text, pt, ct, fin = [], 0, 0, "stop"
                for piece in gen:
                    if isinstance(piece, tuple):
                        _, pt, ct, fin = piece
                    else:
                        text.append(piece)
                self._json(200, {
                    "id": cid, "object": "chat.completion", "created": created, "model": ENGINE.name,
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "".join(text)},
                                 "finish_reason": fin}],
                    "usage": {"prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct},
                })
                log(f"{ct} tokens in {time.time() - t0:.1f}s")
        except ValueError as e:
            self._err(400, str(e))
        except Exception as e:
            log(f"error: {e!r}")
            try:
                self._err(500, f"{type(e).__name__}: {e}")
            except Exception:
                pass
        finally:
            ENGINE.lock.release()


def main():
    global ENGINE
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True, help="local folder or Hugging Face repo id (safetensors)")
    ap.add_argument("--name", default=None, help="model id reported to clients (default: folder name)")
    ap.add_argument("--port", type=int, default=8090)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--compression", choices=["4bit", "8bit", "none"], default="4bit")
    ap.add_argument("--max-seq-len", type=int, default=4096)
    ap.add_argument("--cache-gb", type=float, default=0.0,
                    help="RAM for a static layer cache (0 = off, default). No gain was measured on a 4B model "
                         "(Windows already caches the files); it may help for models near the RAM size.")
    ap.add_argument("--vram-gb", type=float, default=0.0,
                    help="VRAM to hold layers permanently (they are never re-read from disk). 0 = off")
    ap.add_argument("--thinking", action="store_true", help="let the model emit its reasoning (slow)")
    a = ap.parse_args()
    name = a.name or a.model.rstrip("\\/").split("\\")[-1].split("/")[-1]
    ENGINE = Engine(a.model, name, None if a.compression == "none" else a.compression,
                    a.max_seq_len, a.cache_gb, a.thinking, a.vram_gb)
    srv = ThreadingHTTPServer((a.host, a.port), Handler)
    log(f"listening on http://{a.host}:{a.port}  (model id: {name})")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
