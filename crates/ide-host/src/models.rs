//! Find model files and engines on this machine, and suggest launch settings.
//!
//! Everything here is read-only and quick; nothing is downloaded or started.

use serde::Serialize;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 5;
const MAX_FILES: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalModel {
    /// Display name (file stem, or `name:tag` for Ollama).
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub quant: Option<String>,
    /// "folder", "ollama", "lmstudio" or "huggingface".
    pub source: String,
    /// A vision projector file sitting next to the model, if any.
    pub mmproj: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Engine {
    pub name: String,
    pub path: String,
}

/// What the GGUF header says about the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct GgufInfo {
    pub block_count: Option<u32>,
    pub expert_count: Option<u32>,
    pub context_length: Option<u32>,
}

/// Quantisation tag in a file name: Q4_K_M, IQ2_XXS, MXFP4, BF16, F16 ...
pub fn quant_from_name(name: &str) -> Option<String> {
    let up = name.to_ascii_uppercase();
    up.split(|c: char| c == '-' || c == '.' || c == ' ')
        .map(|t| t.trim_matches('_'))
        .find(|t| {
            let digit_after = |p: &str| t.len() > p.len() && t.starts_with(p) && t.as_bytes()[p.len()].is_ascii_digit();
            matches!(*t, "BF16" | "F16" | "F32" | "MXFP4") || digit_after("Q") || digit_after("IQ")
        })
        .map(str::to_string)
}

fn is_gguf(p: &Path) -> bool {
    p.extension().map_or(false, |e| e.eq_ignore_ascii_case("gguf"))
}

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A shard that is not the first of a split model ("...-00002-of-00005.gguf").
fn is_later_shard(name: &str) -> bool {
    if let Some(i) = name.rfind("-of-") {
        let before = &name[..i];
        if let Some(j) = before.rfind('-') {
            let n = &before[j + 1..];
            return n.len() == 5 && n.chars().all(|c| c.is_ascii_digit()) && n != "00001";
        }
    }
    false
}

/// Every `.gguf` model under `dir` (vision projectors are attached to their model, not listed).
pub fn scan_dir(dir: &Path, source: &str) -> Vec<LocalModel> {
    let mut files = Vec::new();
    walk(dir, 0, &mut files);
    let mmprojs: Vec<PathBuf> = files.iter().filter(|p| lower_name(p).contains("mmproj")).cloned().collect();
    let mut out = Vec::new();
    for p in files {
        let ln = lower_name(&p);
        if ln.contains("mmproj") || is_later_shard(&ln) {
            continue;
        }
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mmproj = mmprojs.iter().find(|m| m.parent() == p.parent()).map(|m| m.to_string_lossy().to_string());
        out.push(LocalModel { quant: quant_from_name(&stem), name: stem, path: p.to_string_lossy().to_string(), size_bytes: size, source: source.into(), mmproj });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, depth + 1, out);
        } else if is_gguf(&p) {
            out.push(p);
        } else if ft.is_symlink() && is_gguf(&p) {
            out.push(p);
        }
    }
}

/// Models in Ollama's store (`<root>/manifests/.../<name>/<tag>` pointing at `<root>/blobs/sha256-...`).
pub fn scan_ollama(root: &Path) -> Vec<LocalModel> {
    let mut manifests = Vec::new();
    collect_files(&root.join("manifests"), 0, &mut manifests);
    let mut out = Vec::new();
    for m in manifests {
        let Ok(bytes) = std::fs::read(&m) else { continue };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
        let Some(layers) = v["layers"].as_array() else { continue };
        let Some(layer) = layers.iter().find(|l| l["mediaType"].as_str() == Some("application/vnd.ollama.image.model")) else { continue };
        let Some(digest) = layer["digest"].as_str() else { continue };
        let blob = root.join("blobs").join(digest.replace(':', "-"));
        let Ok(meta) = std::fs::metadata(&blob) else { continue };
        let tag = m.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = m.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(LocalModel {
            name: format!("{name}:{tag}"),
            path: blob.to_string_lossy().to_string(),
            size_bytes: meta.len(),
            quant: quant_from_name(&tag),
            source: "ollama".into(),
            mmproj: None,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn collect_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_files(&p, depth + 1, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// Where other tools keep models, relative to the user's home folder.
pub fn known_roots(home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (home.join(".lmstudio").join("models"), "lmstudio"),
        (home.join(".cache").join("lm-studio").join("models"), "lmstudio"),
        (home.join(".cache").join("huggingface").join("hub"), "huggingface"),
    ]
}

/// Everything found: the user's own folders, the known tool folders, and Ollama's store.
pub fn find_models(home: Option<&Path>, extra_dirs: &[PathBuf]) -> Vec<LocalModel> {
    let mut all: Vec<LocalModel> = Vec::new();
    for d in extra_dirs {
        all.extend(scan_dir(d, "folder"));
    }
    if let Some(h) = home {
        for (d, src) in known_roots(h) {
            all.extend(scan_dir(&d, src));
        }
        all.extend(scan_ollama(&h.join(".ollama").join("models")));
    }
    let mut seen = std::collections::HashSet::new();
    all.retain(|m| seen.insert(m.path.to_lowercase()));
    all
}

/// Look for `llama-server` (and friends): an explicit path first, then PATH, then usual folders.
pub fn find_engines(explicit: Option<&Path>, path_var: Option<&str>, home: Option<&Path>) -> Vec<Engine> {
    let exe = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
    let mut found: Vec<Engine> = Vec::new();
    let mut add = |p: PathBuf| {
        if p.is_file() && !found.iter().any(|e| e.path.eq_ignore_ascii_case(&p.to_string_lossy())) {
            found.push(Engine { name: "llama-server".into(), path: p.to_string_lossy().to_string() });
        }
    };
    if let Some(p) = explicit {
        if p.is_dir() {
            add(p.join(exe));
        } else {
            add(p.to_path_buf());
        }
    }
    if let Some(pv) = path_var {
        for d in std::env::split_paths(pv) {
            add(d.join(exe));
        }
    }
    if let Some(h) = home {
        for sub in ["llama.cpp", "llama.cpp/build/bin", "llama.cpp/build/bin/Release", "llama-cpp", "AppData/Local/llama.cpp"] {
            add(h.join(sub).join(exe));
        }
    }
    found
}

// ---- GGUF header ----

fn rd<R: Read, const N: usize>(r: &mut R) -> Option<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b).ok()?;
    Some(b)
}

fn skip_value<R: Read + Seek>(r: &mut R, ty: u32, depth: u8) -> Option<()> {
    let fixed = |n: i64, r: &mut R| r.seek(SeekFrom::Current(n)).ok().map(|_| ());
    match ty {
        0 | 1 | 7 => fixed(1, r),
        2 | 3 => fixed(2, r),
        4 | 5 | 6 => fixed(4, r),
        10 | 11 | 12 => fixed(8, r),
        8 => {
            let n = u64::from_le_bytes(rd(r)?);
            fixed(i64::try_from(n).ok()?, r)
        }
        9 if depth < 2 => {
            let et = u32::from_le_bytes(rd(r)?);
            let n = u64::from_le_bytes(rd(r)?);
            for _ in 0..n.min(2_000_000) {
                skip_value(r, et, depth + 1)?;
            }
            Some(())
        }
        _ => None,
    }
}

fn read_uint<R: Read>(r: &mut R, ty: u32) -> Option<Option<u64>> {
    Some(match ty {
        2 => Some(u16::from_le_bytes(rd(r)?) as u64),
        4 => Some(u32::from_le_bytes(rd(r)?) as u64),
        10 => Some(u64::from_le_bytes(rd(r)?)),
        _ => None,
    })
}

/// Read layer count, expert count and trained context length from a GGUF header. `None` if not a GGUF.
pub fn read_gguf_info(path: &Path) -> Option<GgufInfo> {
    let f = std::fs::File::open(path).ok()?;
    let mut r = BufReader::new(f);
    if &rd::<_, 4>(&mut r)? != b"GGUF" {
        return None;
    }
    let _version = u32::from_le_bytes(rd(&mut r)?);
    let _tensors = u64::from_le_bytes(rd(&mut r)?);
    let kvs = u64::from_le_bytes(rd(&mut r)?).min(100_000);
    let mut info = GgufInfo::default();
    for _ in 0..kvs {
        let klen = u64::from_le_bytes(rd(&mut r)?);
        if klen > 256 {
            return Some(info);
        }
        let mut kb = vec![0u8; klen as usize];
        r.read_exact(&mut kb).ok()?;
        let key = String::from_utf8_lossy(&kb).to_string();
        let ty = u32::from_le_bytes(rd(&mut r)?);
        if key.starts_with("tokenizer.") {
            return Some(info); // large arrays; everything we need comes earlier
        }
        let slot = if key.ends_with(".block_count") {
            Some(0)
        } else if key.ends_with(".expert_count") {
            Some(1)
        } else if key.ends_with(".context_length") {
            Some(2)
        } else {
            None
        };
        match slot {
            Some(s) => match read_uint(&mut r, ty)? {
                Some(v) => {
                    let v = v.min(u32::MAX as u64) as u32;
                    match s {
                        0 => info.block_count = Some(v),
                        1 => info.expert_count = Some(v),
                        _ => info.context_length = Some(v),
                    }
                }
                None => skip_value(&mut r, ty, 0)?,
            },
            None => skip_value(&mut r, ty, 0)?,
        }
    }
    Some(info)
}

/// Suggested launch settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Suggestion {
    /// Layers on the GPU (99 = all).
    pub ngl: u32,
    /// Mixture-of-experts layers whose experts stay in system memory (0 = none).
    pub n_cpu_moe: u32,
    pub ctx: u32,
    pub fits_gpu: bool,
}

/// A starting point, not a guarantee: leave ~1.5 GiB of video memory for the KV cache and compute.
pub fn suggest(size_bytes: u64, vram_mib: Option<u32>, info: &GgufInfo) -> Suggestion {
    let ctx = info.context_length.map_or(16384, |c| c.min(16384)).max(2048);
    let blocks = info.block_count.unwrap_or(0);
    let Some(vram) = vram_mib else {
        return Suggestion { ngl: 0, n_cpu_moe: 0, ctx, fits_gpu: false };
    };
    let budget = (vram as f64 - 1536.0).max(0.0) * 1_048_576.0;
    let size = size_bytes as f64;
    if size <= budget {
        return Suggestion { ngl: 99, n_cpu_moe: 0, ctx, fits_gpu: true };
    }
    let spill = 1.0 - budget / size.max(1.0);
    if info.expert_count.unwrap_or(0) > 1 && blocks > 0 {
        // MoE: keep attention on the GPU, push the experts of the first N layers to the CPU.
        let n = ((blocks as f64) * spill).ceil() as u32;
        return Suggestion { ngl: 99, n_cpu_moe: n.min(blocks), ctx, fits_gpu: false };
    }
    let ngl = if blocks > 0 { ((blocks as f64) * (budget / size)).floor() as u32 } else { 0 };
    Suggestion { ngl, n_cpu_moe: 0, ctx, fits_gpu: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn quant_tags() {
        assert_eq!(quant_from_name("Qwen3-35B-A3B-Q4_K_M").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_from_name("model-IQ2_XXS").as_deref(), Some("IQ2_XXS"));
        assert_eq!(quant_from_name("gpt-oss-120b-MXFP4").as_deref(), Some("MXFP4"));
        assert_eq!(quant_from_name("x.BF16").as_deref(), Some("BF16"));
        assert_eq!(quant_from_name("plain-model"), None);
    }

    #[test]
    fn scan_skips_projectors_and_later_shards_and_attaches_mmproj() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        for n in ["m-Q4_K_M.gguf", "mmproj-m.gguf", "big-00001-of-00002.gguf", "big-00002-of-00002.gguf", "notes.txt"] {
            std::fs::write(sub.join(n), vec![0u8; 10]).unwrap();
        }
        let v = scan_dir(d.path(), "folder");
        let names: Vec<_> = v.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["big-00001-of-00002", "m-Q4_K_M"]);
        assert!(v[1].mmproj.as_deref().unwrap().ends_with("mmproj-m.gguf"));
        assert_eq!(v[1].size_bytes, 10);
        assert_eq!(v[1].quant.as_deref(), Some("Q4_K_M"));
        assert!(scan_dir(&d.path().join("missing"), "folder").is_empty());
    }

    #[test]
    fn ollama_store_is_read_without_ollama_running() {
        let d = tempfile::tempdir().unwrap();
        let mdir = d.path().join("manifests/registry.ollama.ai/library/llama3");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::create_dir_all(d.path().join("blobs")).unwrap();
        std::fs::write(d.path().join("blobs/sha256-abc"), vec![0u8; 7]).unwrap();
        let manifest = r#"{"layers":[{"mediaType":"application/vnd.ollama.image.template","digest":"sha256:zzz"},{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:abc"}]}"#;
        std::fs::write(mdir.join("8b"), manifest).unwrap();
        let v = scan_ollama(d.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "llama3:8b");
        assert_eq!(v[0].size_bytes, 7);
        assert_eq!(v[0].source, "ollama");
    }

    #[test]
    fn engines_come_from_explicit_path_and_path_variable() {
        let d = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        std::fs::write(d.path().join(exe), b"x").unwrap();
        let pv = d.path().to_string_lossy().to_string();
        let a = find_engines(None, Some(&pv), None);
        assert_eq!(a.len(), 1);
        let b = find_engines(Some(d.path()), Some(&pv), None);
        assert_eq!(b.len(), 1, "same file is listed once");
        assert!(find_engines(None, Some("Z:/nowhere"), None).is_empty());
    }

    fn kv_str(buf: &mut Vec<u8>, k: &str, v: &str) {
        buf.extend((k.len() as u64).to_le_bytes());
        buf.extend(k.as_bytes());
        buf.extend(8u32.to_le_bytes());
        buf.extend((v.len() as u64).to_le_bytes());
        buf.extend(v.as_bytes());
    }
    fn kv_u32(buf: &mut Vec<u8>, k: &str, v: u32) {
        buf.extend((k.len() as u64).to_le_bytes());
        buf.extend(k.as_bytes());
        buf.extend(4u32.to_le_bytes());
        buf.extend(v.to_le_bytes());
    }

    #[test]
    fn gguf_header_gives_layers_experts_and_context() {
        let mut b = Vec::new();
        b.extend(b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(4u64.to_le_bytes());
        kv_str(&mut b, "general.architecture", "qwen3moe");
        kv_u32(&mut b, "qwen3moe.block_count", 40);
        kv_u32(&mut b, "qwen3moe.expert_count", 128);
        kv_u32(&mut b, "qwen3moe.context_length", 262144);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.gguf");
        std::fs::File::create(&p).unwrap().write_all(&b).unwrap();
        let i = read_gguf_info(&p).unwrap();
        assert_eq!((i.block_count, i.expert_count, i.context_length), (Some(40), Some(128), Some(262144)));
        std::fs::write(&p, b"not a gguf").unwrap();
        assert!(read_gguf_info(&p).is_none());
        assert!(read_gguf_info(&d.path().join("none.gguf")).is_none());
    }

    #[test]
    fn suggestions_fit_small_spill_experts_or_layers() {
        let gib = 1024u64 * 1024 * 1024;
        let moe = GgufInfo { block_count: Some(40), expert_count: Some(128), context_length: Some(262144) };
        let dense = GgufInfo { block_count: Some(60), expert_count: None, context_length: Some(8192) };
        let s = suggest(4 * gib, Some(8188), &dense);
        assert!(s.fits_gpu && s.ngl == 99 && s.n_cpu_moe == 0);
        assert_eq!(s.ctx, 8192);
        let s = suggest(23 * gib, Some(8188), &moe);
        assert!(!s.fits_gpu && s.ngl == 99 && s.n_cpu_moe > 20 && s.n_cpu_moe <= 40);
        assert_eq!(s.ctx, 16384);
        let s = suggest(16 * gib, Some(8188), &dense);
        assert!(!s.fits_gpu && s.ngl > 0 && s.ngl < 60 && s.n_cpu_moe == 0);
        let s = suggest(4 * gib, None, &dense);
        assert_eq!(s.ngl, 0);
    }
}
