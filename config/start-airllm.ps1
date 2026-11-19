# Starts the AirLLM server (OpenAI-compatible) so the IDE can use big safetensors models on a small GPU.
# In the IDE: Settings > Model server address = http://127.0.0.1:8090 (no API key needed).
# One-time setup of the Python environment (Python 3.11): see tools/airllm-server/README.md
param(
  [Parameter(Mandatory = $true)][string]$Model,          # folder with the original safetensors model
  [string]$Python = "E:\airllm-test\venv\Scripts\python.exe",
  [int]$Port = 8090,
  [ValidateSet("4bit", "8bit", "none")][string]$Compression = "4bit",
  [int]$MaxSeqLen = 4096,
  [double]$CacheGb = 0
)
$server = Join-Path $PSScriptRoot "..\tools\airllm-server\server.py"
if (-not (Test-Path $Python)) { Write-Error "Python venv not found: $Python (see tools/airllm-server/README.md)"; exit 1 }
if (-not (Test-Path $Model)) { Write-Error "Model folder not found: $Model"; exit 1 }
$env:PYTHONIOENCODING = "utf-8"
& $Python $server --model $Model --port $Port --compression $Compression --max-seq-len $MaxSeqLen --cache-gb $CacheGb
