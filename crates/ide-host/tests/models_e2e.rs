//! Real-machine check, run on demand:
//! ASA_E2E_ENGINE=<llama-server> ASA_E2E_MODELS=<folder> cargo test -p ide-host --test models_e2e -- --ignored --nocapture
use ide_host::launcher::{LaunchOpts, ModelServer};
use ide_host::models::{find_models, read_gguf_info, suggest};
use std::path::PathBuf;

#[tokio::test]
#[ignore]
async fn find_read_launch_stop() {
    let engine = std::env::var("ASA_E2E_ENGINE").expect("ASA_E2E_ENGINE");
    let dir = PathBuf::from(std::env::var("ASA_E2E_MODELS").expect("ASA_E2E_MODELS"));
    let found = find_models(None, &[dir]);
    assert!(!found.is_empty(), "no models found");
    for m in &found {
        let i = read_gguf_info(std::path::Path::new(&m.path));
        println!("{} {} MB quant={:?} info={:?} suggest={:?}", m.name, m.size_bytes / 1_000_000, m.quant, i, suggest(m.size_bytes, Some(8188), &i.unwrap_or_default()));
    }
    let small = found.iter().min_by_key(|m| m.size_bytes).unwrap();
    let srv = ModelServer::new();
    srv.start(LaunchOpts { backend: "llamacpp".into(), server: None, compression: None, vram_gb: None, engine, model: small.path.clone(), alias: "e2e".into(), mmproj: None, ctx: 4096, ngl: 99, n_cpu_moe: 0, port: 18080 }).await.unwrap();
    let mut ready = false;
    for _ in 0..90 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let st = srv.status();
        if st.state == "ready" { ready = true; break; }
        assert_ne!(st.state, "exited", "server exited: {:?}", st.log);
    }
    println!("ready={ready}");
    assert!(ready);
    srv.stop().await;
    assert_eq!(srv.status().state, "stopped");
}

#[test]
#[ignore]
fn engines_on_this_machine() {
    let mounts = ide_host::models::mount_points();
    println!("mounts={mounts:?}");
    let e = ide_host::models::find_engines(None, std::env::var("PATH").ok().as_deref(), None, &mounts);
    println!("engines={e:?}");
}

/// AirLLM through the same launcher the IDE uses.
/// ASA_E2E_PY=<python> ASA_E2E_SERVER=<server.py> ASA_E2E_ST_MODEL=<model folder>
#[tokio::test]
#[ignore]
async fn airllm_start_chat_stop() {
    let py = std::env::var("ASA_E2E_PY").expect("ASA_E2E_PY");
    let server = std::env::var("ASA_E2E_SERVER").expect("ASA_E2E_SERVER");
    let model = std::env::var("ASA_E2E_ST_MODEL").expect("ASA_E2E_ST_MODEL");
    let srv = ModelServer::new();
    let t0 = std::time::Instant::now();
    srv.start(LaunchOpts { backend: "airllm".into(), server: Some(server), compression: Some("4bit".into()), vram_gb: Some(4.0), engine: py, model, alias: "e2e-air".into(), mmproj: None, ctx: 2048, ngl: 0, n_cpu_moe: 0, port: 18090 }).await.unwrap();
    let mut ready = false;
    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let st = srv.status();
        if st.state == "ready" { ready = true; break; }
        assert_ne!(st.state, "exited", "server exited: {:?}", st.log);
    }
    println!("ready={ready} after {}s", t0.elapsed().as_secs());
    assert!(ready);
    let body = serde_json::json!({"messages":[{"role":"user","content":"Say hi in three words."}],"max_tokens":8,"temperature":0});
    let t = std::time::Instant::now();
    let r: serde_json::Value = ureq::post("http://127.0.0.1:18090/v1/chat/completions").timeout(std::time::Duration::from_secs(600)).send_json(body).unwrap().into_json().unwrap();
    println!("chat in {}s: {} usage={}", t.elapsed().as_secs(), r["choices"][0]["message"]["content"], r["usage"]);
    println!("log tail: {:?}", srv.status().log.iter().rev().take(6).collect::<Vec<_>>());
    srv.stop().await;
    assert_eq!(srv.status().state, "stopped");
}
