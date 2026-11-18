//! Persistent user settings (`<dir>/settings.json`).

use serde::{Deserialize, Deserializer, Serialize};
use std::io::Write;
use std::path::Path;

const FILE: &str = "settings.json";
const MAX_RECENT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub endpoint: String,
    pub model: String,
    pub ignore: Vec<String>,
    pub theme: String,
    #[serde(deserialize_with = "de_font_size")]
    pub font_size: u8,
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            endpoint: "http://127.0.0.1:11434".into(),
            model: String::new(),
            ignore: Vec::new(),
            theme: "harbor".into(),
            font_size: 13,
            recent: Vec::new(),
        }
    }
}

fn de_font_size<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    // Any JSON number is accepted (floats round half away from zero, so
    // 13.5 -> 14, then clamp 8..=32); non-numeric values fall back to 13.
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v.as_f64() {
        Some(f) if f.is_finite() => f.round().clamp(8.0, 32.0) as u8,
        _ => 13,
    })
}

impl Settings {
    /// Load `<dir>/settings.json`. Missing file -> defaults. An unparseable
    /// file yields defaults and is renamed to `settings.json.bad`
    /// (replacing an older one) so the user's data is not silently lost.
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Settings::default(),
        };
        match serde_json::from_slice::<Settings>(&bytes) {
            Ok(mut s) => {
                s.font_size = s.font_size.clamp(8, 32);
                let mut seen = Vec::new();
                for r in std::mem::take(&mut s.recent) {
                    if !seen.contains(&r) && seen.len() < MAX_RECENT {
                        seen.push(r);
                    }
                }
                s.recent = seen;
                s
            }
            Err(_) => {
                let bad = dir.join("settings.json.bad");
                let _ = std::fs::remove_file(&bad);
                let _ = std::fs::rename(&path, &bad);
                Settings::default()
            }
        }
    }

    /// Atomic save: write a temp file in `dir`, fsync, rename over the target.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("{FILE}.{}.{n}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let res = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, dir.join(FILE))
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    }

    /// Move `path` to the front of `recent`, de-duplicated, max 10 entries.
    pub fn push_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MAX_RECENT);
    }
}
