# Runs a large GGUF model (for example a 40B Q4_K_M) with llama.cpp on an 8 GB GPU / 16 GB RAM machine.
# The file (24 GB) is larger than RAM, so most of it is paged in from the SSD (--mmap, the default):
# expect roughly 1 token/second or less. Raise -Layers until the GPU is nearly full (watch nvidia-smi);
# lower it if you see out-of-memory errors.
param(
  [Parameter(Mandatory = $true)][string]$Model,       # path of a large GGUF file
  [string]$Server = "llama-server",                    # on PATH, or give the full path
  [int]$Layers = 10,
  [int]$Ctx = 8192,
  [int]$Port = 8080
)
if (-not (Test-Path $Model)) { Write-Error "Model not found (is the download finished?): $Model"; exit 1 }
& $Server --model $Model --port $Port --host 127.0.0.1 -ngl $Layers -c $Ctx --flash-attn on --jinja
# In the IDE: Settings > Model server address = http://127.0.0.1:8080 (no API key needed).
