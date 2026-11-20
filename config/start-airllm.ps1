# Starts the AirLLM server (OpenAI-compatible) so the IDE can use big safetensors models on a small GPU.
# In the IDE: Settings > Model server address = http://127.0.0.1:8090 (no API key needed).
# One-time setup of the Python environment (Python 3.11): see tools/airllm-server/README.md
param(
  [Parameter(Mandatory = $true)][string]$Model,          # folder with the original safetensors model
  # Python of the environment where airllm is installed (see tools/airllm-server/README.md). Defaults to $env:ASA_AIRLLM_PYTHON.
  [string]$Python = $env:ASA_AIRLLM_PYTHON,
  [int]$Port = 8090,
  [ValidateSet("4bit", "8bit", "none")][string]$Compression = "4bit",
  [int]$MaxSeqLen = 4096,
  [double]$CacheGb = 0,
  [double]$VramGb = 0                                      # VRAM to keep layers on the GPU permanently
)
$server = Join-Path $PSScriptRoot "..\tools\airllm-server\server.py"
if (-not $Python -or -not (Test-Path $Python)) { Write-Error "Python venv not found: $Python (see tools/airllm-server/README.md)"; exit 1 }
if (-not (Test-Path $Model)) { Write-Error "Model folder not found: $Model"; exit 1 }
$env:PYTHONIOENCODING = "utf-8"
& $Python $server --model $Model --port $Port --compression $Compression --max-seq-len $MaxSeqLen --cache-gb $CacheGb --vram-gb $VramGb
