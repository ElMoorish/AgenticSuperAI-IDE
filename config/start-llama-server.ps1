# Tier 1 (interactive agent) on RTX 4060 8 GB. Flags from the merged architecture spec.
# Usage: .\config\start-llama-server.ps1 -Model <path\to\model.gguf>
# For a 16-30B MoE model add:  --n-cpu-moe 24 --moe-cache-mib 1024 -lm mlock
# (verify those flags exist in your llama.cpp build: llama-server --help)
param(
  [Parameter(Mandatory = $true)][string]$Model,
  [int]$Ctx = 16384,
  [int]$Port = 8080
)

llama-server `
  -m $Model `
  -ngl 99 `
  --ctx-size $Ctx `
  --flash-attn on `
  -ctk q8_0 -ctv q8_0 `
  --jinja `
  --host 127.0.0.1 --port $Port
