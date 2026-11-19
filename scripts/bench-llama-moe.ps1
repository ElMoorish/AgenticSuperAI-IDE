# Benchmarks one MoE GGUF under different expert-offload settings with llama-bench.
# Usage: .\scripts\bench-llama-moe.ps1 -Model E:\models\olmoe\olmoe-1b-7b-0924-instruct-q4_k_m.gguf
# Prints prompt-processing (pp) and token-generation (tg) tokens/s per configuration.
param(
  [Parameter(Mandatory = $true)][string]$Model,
  [string]$Bench = "E:\tools\llama.cpp\llama-bench.exe",
  [int]$Prompt = 128,
  [int]$Gen = 32,
  [int]$Repeats = 3,
  # Each entry: label | extra llama-bench arguments
  [string[]]$Configs = @(
    "all-on-GPU|-ngl 99",
    "experts-on-CPU (attention on GPU)|-ngl 99 -ncmoe 99",
    "half-the-expert-layers-on-CPU|-ngl 99 -ncmoe 8",
    "CPU only|-ngl 0"
  )
)
if (-not (Test-Path $Model)) { Write-Error "Model not found: $Model"; exit 1 }
foreach ($c in $Configs) {
  $label, $args_ = $c -split "\|", 2
  Write-Host "=== $label  ($args_)"
  & $Bench -m $Model -p $Prompt -n $Gen -r $Repeats -fa 1 -o md @($args_ -split " ")
}
