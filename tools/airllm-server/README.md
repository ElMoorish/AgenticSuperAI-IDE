# AirLLM server

An OpenAI-compatible server (`/v1/chat/completions`, streaming or not, `/v1/models`, `/health`) around
[AirLLM](https://github.com/lyogavin/airllm), which streams one layer at a time through the GPU. It lets the IDE
use models far bigger than the GPU's memory, at a large cost in speed.

## One-time setup (Windows, Python 3.11, NVIDIA GPU)

```powershell
py -3.11 -m venv E:\airllm-test\venv
E:\airllm-test\venv\Scripts\python.exe -m pip install torch --index-url https://download.pytorch.org/whl/cu124
E:\airllm-test\venv\Scripts\python.exe -m pip install airllm "transformers>=5.8" accelerate safetensors huggingface-hub scipy sentencepiece tqdm bitsandbytes
```

The model must be the original Hugging Face **safetensors** (not GGUF). The first start writes a layer-split copy
next to it (4-bit: about 30% of the original size).

## Run

```powershell
.\config\start-airllm.ps1 -Model E:\airllm-test\Qwen3.5-4B
```

Then in the IDE: Settings > Model server address = `http://127.0.0.1:8090`. Check with `python smoke_test.py`.

## What it changes compared with stock AirLLM

* `clean_memory()` (a full `gc.collect()` plus `empty_cache()` after every layer) is replaced by `empty_cache()` only.
  Measured on a 4B model in 4-bit: 7.8 -> 1.6 s per token (the cleanup was 66% of the time).
* Optional static layer cache in RAM (`--cache-gb`). Off by default: no gain was measured on the 4B model,
  because Windows already caches the files.

## Limits

* One request at a time (a second one gets HTTP 503).
* Speed is roughly the compressed model size divided by disk speed, per generated token. Measured: Qwen3.5-4B,
  4-bit, about 1.8 s per token. A 27B is expected to be tens of seconds per token (estimate, not measured).
* Reasoning ("thinking") is off by default (`--thinking` turns it on).
* Not tested yet inside the IDE's agent loop.
