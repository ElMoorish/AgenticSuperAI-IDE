use ide_host::probe::*;
use ide_host::settings::Settings;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[test]
fn settings_defaults_on_corrupt_file_and_keeps_bad_copy() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(Settings::load(d.path()), Settings::default());
    std::fs::write(d.path().join("settings.json"), "{ not json").unwrap();
    let s = Settings::load(d.path());
    assert_eq!(s, Settings::default());
    assert_eq!(s.endpoint, "http://127.0.0.1:11434");
    assert_eq!(s.theme, "harbor");
    assert_eq!(s.font_size, 13);
    assert_eq!(std::fs::read_to_string(d.path().join("settings.json.bad")).unwrap(), "{ not json");
    assert!(!d.path().join("settings.json").exists());
    // a second corrupt file overwrites the older .bad
    std::fs::write(d.path().join("settings.json"), "also bad").unwrap();
    Settings::load(d.path());
    assert_eq!(std::fs::read_to_string(d.path().join("settings.json.bad")).unwrap(), "also bad");
}

#[test]
fn settings_partial_file_loads_with_defaults() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("settings.json"), r#"{"model":"qwen"}"#).unwrap();
    let s = Settings::load(d.path());
    assert_eq!(s.model, "qwen");
    assert_eq!(s.theme, "harbor");
}

#[test]
fn settings_save_is_atomic() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("nested").join("cfg");
    let mut s = Settings::default();
    s.model = "m1".into();
    s.save(&dir).unwrap();
    s.model = "m2".into();
    s.push_recent("C:/a");
    s.save(&dir).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()], "no temp left: {names:?}");
    assert_eq!(Settings::load(&dir), s);
}

#[test]
fn settings_clamps_font_size_and_dedupes_recent() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("settings.json"), r#"{"font_size":200}"#).unwrap();
    assert_eq!(Settings::load(d.path()).font_size, 32);
    std::fs::write(d.path().join("settings.json"), r#"{"font_size":2}"#).unwrap();
    assert_eq!(Settings::load(d.path()).font_size, 8);
    let mut s = Settings::default();
    for i in 0..12 {
        s.push_recent(&format!("p{i}"));
    }
    s.push_recent("p5");
    assert_eq!(s.recent.len(), 10);
    assert_eq!(s.recent[0], "p5");
    assert_eq!(s.recent.iter().filter(|p| *p == "p5").count(), 1);
    assert_eq!(s.recent[1], "p11");
}

fn serve(body: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 2048];
            let mut got = Vec::new();
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let ok = got.starts_with(b"GET /v1/models");
            let resp = if ok {
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            };
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

#[test]
fn probe_parses_models_from_mock_server() {
    let p = serve(r#"{"object":"list","data":[{"id":"qwen2.5"},{"id":"llama3"}]}"#);
    let r = probe_endpoint(&format!("http://127.0.0.1:{p}"));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.models, vec!["qwen2.5", "llama3"]);
    assert!(r.error.is_none());
    let p = serve(r#"{"models":[{"name":"mistral:7b"},{"name":"phi3"}]}"#);
    let r = probe_endpoint(&format!("http://127.0.0.1:{p}/"));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.models, vec!["mistral:7b", "phi3"]);
}

#[test]
fn probe_accepts_base_with_v1_suffix() {
    let p = serve(r#"{"data":[{"id":"a"}]}"#);
    for base in [format!("http://127.0.0.1:{p}/v1"), format!("http://127.0.0.1:{p}/v1/")] {
        let r = probe_endpoint(&base);
        assert!(r.ok, "{base}: {:?}", r.error);
        assert_eq!(r.models, vec!["a"]);
    }
}

fn dead_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn probe_reports_error_when_down() {
    let r = probe_endpoint(&format!("http://127.0.0.1:{}", dead_port()));
    assert!(!r.ok);
    assert!(r.models.is_empty());
    assert!(r.error.is_some_and(|e| !e.is_empty()));
    let r = probe_endpoint("not a url");
    assert!(!r.ok && r.error.is_some());
}

#[test]
fn probe_strips_credentials_from_error() {
    let r = probe_endpoint("http://user:pass@127.0.0.1:1");
    assert!(!r.ok);
    let e = r.error.unwrap();
    assert!(!e.contains("user") && !e.contains("pass"), "{e}");
}

#[test]
fn probe_times_out() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let _h = std::thread::spawn(move || {
        let mut keep = Vec::new();
        for s in l.incoming().flatten() {
            keep.push(s);
        }
    });
    let t = Instant::now();
    let r = probe_endpoint(&format!("http://127.0.0.1:{port}"));
    assert!(!r.ok);
    assert!(t.elapsed() < Duration::from_secs(5), "took {:?}", t.elapsed());
}

#[test]
fn detect_servers_returns_only_answering() {
    let up1 = serve(r#"{"data":[]}"#);
    let up2 = serve(r#"{"data":[]}"#);
    let down = dead_port();
    let u = |p: u16| format!("http://127.0.0.1:{p}");
    let (a, b, c) = (u(up1), u(down), u(up2));
    let found = detect_servers_at(&[("One", a.as_str()), ("Two", b.as_str()), ("Three", c.as_str())]);
    assert_eq!(
        found,
        vec![
            DetectedServer { name: "One".into(), url: a },
            DetectedServer { name: "Three".into(), url: c }
        ]
    );
}

#[test]
fn gpu_parse_handles_csv_line() {
    assert_eq!(parse_gpu_csv("5821, 8188"), Some((5821, 8188)));
    assert_eq!(parse_gpu_csv("5821, 8188\n"), Some((5821, 8188)));
    assert_eq!(parse_gpu_csv("5821, 8188\r\n"), Some((5821, 8188)));
    assert_eq!(parse_gpu_csv("100, 200\n300, 400\n"), Some((100, 200)));
    assert_eq!(parse_gpu_csv("garbage"), None);
    assert_eq!(parse_gpu_csv("a, b"), None);
    assert_eq!(parse_gpu_csv(""), None);
    assert_eq!(parse_gpu_csv("[N/A], 8188"), None);
}

fn serve_raw(resp: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 2048];
            let mut got = Vec::new();
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

const R302: &str = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const RHTML: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<html>hi</html";
const RNODATA: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";

#[test]
fn probe_rejects_redirect_html_and_wrong_json() {
    for (name, raw) in [("302", R302), ("html", RHTML), ("nodata", RNODATA)] {
        let p = serve_raw(raw);
        let r = probe_endpoint(&format!("http://127.0.0.1:{p}"));
        assert!(!r.ok, "{name} must not be ok");
        let e = r.error.expect(name);
        assert!(!e.contains("127.0.0.1:9"), "{e}");
    }
}

#[test]
fn probe_rejects_over_cap_body() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let n = 5 * 1024 * 1024;
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n{{\"data\":[").as_bytes());
            let chunk = vec![b' '; 64 * 1024];
            for _ in 0..(n / chunk.len()) {
                if s.write_all(&chunk).is_err() {
                    break;
                }
            }
        }
    });
    let r = probe_endpoint(&format!("http://127.0.0.1:{port}"));
    assert!(!r.ok);
}

#[test]
fn detect_servers_excludes_non_model_servers() {
    let good = serve(r#"{"data":[]}"#);
    let (a, b, c, d) = (serve_raw(R302), serve_raw(RHTML), serve_raw(RNODATA), format!("http://127.0.0.1:{good}"));
    let (a, b, c) = (format!("http://127.0.0.1:{a}"), format!("http://127.0.0.1:{b}"), format!("http://127.0.0.1:{c}"));
    let found = detect_servers_at(&[("a", &a), ("b", &b), ("c", &c), ("good", &d)]);
    assert_eq!(found, vec![DetectedServer { name: "good".into(), url: d }]);
}

#[test]
fn settings_concurrent_saves_do_not_collide() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    let hs: Vec<_> = (0..4)
        .map(|t| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    let mut s = Settings::default();
                    s.model = format!("m{t}-{i}");
                    s.save(&dir).unwrap();
                }
            })
        })
        .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
    assert!(v["model"].as_str().unwrap().starts_with('m'));
}

#[test]
fn settings_odd_font_size_is_not_corruption() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("settings.json");
    // floats round (half away from zero) then clamp: 13.5 -> 14
    std::fs::write(&f, r#"{"font_size":13.5,"model":"keep"}"#).unwrap();
    let s = Settings::load(d.path());
    assert_eq!((s.font_size, s.model.as_str()), (14, "keep"));
    std::fs::write(&f, r#"{"font_size":"big","model":"keep"}"#).unwrap();
    let s = Settings::load(d.path());
    assert_eq!((s.font_size, s.model.as_str()), (13, "keep"));
    assert!(!d.path().join("settings.json.bad").exists());
}
