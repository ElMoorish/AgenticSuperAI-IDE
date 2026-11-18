//! Pre-flight VRAM budget (Doc A section B.5): weights + KV cache + compute buffer must fit
//! under a safe ceiling, otherwise apply the downgrade cascade before loading.

/// ~1.2 GB of the 8 GB RTX 4060 goes to the Windows desktop (DWM, browser, IDE).
pub const SAFE_VRAM_MIB_8GB: u64 = 6800;

#[derive(Clone, Debug)]
pub struct ModelProfile {
    pub gpu_weights_mib: u64,
    pub n_layers_gpu: u64,
    pub n_head_kv: u64,
    pub head_dim: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadPlan {
    pub ctx: u64,
    pub kv_bytes_per_elem: f64, // 2.0 = f16, ~1.0625 = q8_0
    pub flash_attn: bool,
    pub moe_cache_mib: u64,
}

/// 2 (K and V) * layers * kv_heads * head_dim * ctx * bytes, in MiB.
pub fn kv_mib(p: &ModelProfile, ctx: u64, bytes_per_elem: f64) -> u64 {
    let bytes = 2.0 * p.n_layers_gpu as f64 * p.n_head_kv as f64 * p.head_dim as f64 * ctx as f64 * bytes_per_elem;
    (bytes / (1024.0 * 1024.0)).ceil() as u64
}

pub fn estimate_mib(p: &ModelProfile, plan: &LoadPlan) -> u64 {
    let compute = if plan.flash_attn { 300 } else { 550 };
    p.gpu_weights_mib + kv_mib(p, plan.ctx, plan.kv_bytes_per_elem) + compute + plan.moe_cache_mib
}

/// Apply steps (1) flash-attn + q8_0 KV, (2) smaller context. Steps (3) CPU offload and
/// (4) queue-to-batch need the runtime adapter, so we report `None` and let the caller decide.
pub fn fit(p: &ModelProfile, mut plan: LoadPlan, ceiling_mib: u64) -> Option<LoadPlan> {
    if estimate_mib(p, &plan) <= ceiling_mib {
        return Some(plan);
    }
    plan.flash_attn = true;
    plan.kv_bytes_per_elem = 1.0625;
    for ctx in [plan.ctx, 16384, 8192, 4096] {
        if ctx > plan.ctx {
            continue;
        }
        plan.ctx = ctx;
        if estimate_mib(p, &plan) <= ceiling_mib {
            return Some(plan);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Qwen2.5-Coder-7B-like: 28 layers, 4 KV heads, head_dim 128, ~4.7 GB Q4_K_M
    fn p7b() -> ModelProfile {
        ModelProfile { gpu_weights_mib: 4700, n_layers_gpu: 28, n_head_kv: 4, head_dim: 128 }
    }

    #[test]
    fn kv_math_matches_hand_calc() {
        // 2*28*4*128*16384*2 bytes = 896 MiB (f16)
        assert_eq!(kv_mib(&p7b(), 16384, 2.0), 896);
    }

    #[test]
    fn q8_kv_roughly_halves() {
        let f16 = kv_mib(&p7b(), 16384, 2.0);
        let q8 = kv_mib(&p7b(), 16384, 1.0625);
        assert!(q8 * 2 < f16 + 100 && q8 < f16);
    }

    #[test]
    fn seven_b_fits_at_16k() {
        let plan = LoadPlan { ctx: 16384, kv_bytes_per_elem: 2.0, flash_attn: false, moe_cache_mib: 0 };
        assert!(fit(&p7b(), plan, SAFE_VRAM_MIB_8GB).is_some());
    }

    #[test]
    fn oversized_model_is_refused() {
        let big = ModelProfile { gpu_weights_mib: 9000, ..p7b() };
        let plan = LoadPlan { ctx: 16384, kv_bytes_per_elem: 2.0, flash_attn: false, moe_cache_mib: 0 };
        assert!(fit(&big, plan, SAFE_VRAM_MIB_8GB).is_none());
    }

    #[test]
    fn cascade_shrinks_when_tight() {
        let tight = ModelProfile { gpu_weights_mib: 5900, ..p7b() };
        let plan = LoadPlan { ctx: 32768, kv_bytes_per_elem: 2.0, flash_attn: false, moe_cache_mib: 0 };
        let out = fit(&tight, plan, SAFE_VRAM_MIB_8GB).expect("should fit after downgrade");
        assert!(out.flash_attn && out.ctx < 32768);
    }
}
