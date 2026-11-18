//! PRD 4 X5 through the whole app: slow Explorer questions are logged and listed for the Data page (with the stress
//! switch), and malformed requests to the Explorer never break the server.

use super::*;
use crate::routes::explore_tests::{Rng, fixture, scratch};
use axum::http::Request;
use serde_json::{Value, json};

/// The fixture database and the app around it. `slow_ms` 0 logs every question.
fn make_app(name: &str, slow_ms: u64, dev: bool) -> (Arc<App>, PathBuf) {
    let dir = scratch(name);
    let db = fixture(&dir, 25, 17);
    let write = tft_store::open(&db).unwrap();
    let calendar = tft_store::load_calendar(&write).unwrap();
    let write = Arc::new(Mutex::new(write));
    let config = Config::from_lookup(|_| None);
    let me = Arc::new(me::Me::new(Arc::clone(&write), &config, calendar));
    let app = Arc::new(App {
        pool: Pool { path: db.clone(), idle: Mutex::new(Vec::new()) },
        comps: Mutex::default(),
        icons: dir.join("icons"),
        dist: None,
        http: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(1)).build().unwrap(),
        write,
        me,
        config,
        dev,
        explorer: tft_explore::Explorer::new(&db, tft_explore::Mode::Wait),
        slow_ms,
    });
    (app, dir)
}

async fn call(app: &Arc<App>, method: Method, uri: &str, body: Vec<u8>) -> (u16, Value) {
    let req = Request::builder().method(method).uri(uri).body(Body::from(body)).unwrap();
    let res = handle(State(Arc::clone(app)), req).await;
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &Arc<App>, uri: &str) -> Value {
    let (status, body) = call(app, Method::GET, uri, Vec::new()).await;
    assert_eq!(status, 200, "{uri}");
    body
}

/// The log is written after the answer, on another thread: wait for it.
async fn slow_list(app: &Arc<App>, at_least: usize) -> Value {
    for _ in 0..100 {
        let b = get(app, "/api/slow-queries").await;
        if b["queries"].as_array().unwrap().len() >= at_least {
            return b;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    get(app, "/api/slow-queries").await
}

#[tokio::test]
async fn slow_questions_are_logged_and_listed_newest_first() {
    // Threshold forced to 0 ms: every question that counts boards is slow.
    let (app, dir) = make_app("explore-slow", 0, false);
    let empty = get(&app, "/api/slow-queries").await;
    assert_eq!(empty["queries"], json!([]));
    assert_eq!(empty["thresholdMs"], json!(0));

    let a = get(&app, "/api/explore?units=TFT16_U01&bucket=MASTER%2B&window=3d").await;
    assert!(a["summary"]["games"].as_i64().unwrap() > 0);
    slow_list(&app, 1).await;
    get(&app, "/api/explore?units=TFT16_U02,TFT16_U03&traits=TFT16_T01&patch=16.1").await;
    get(&app, "/api/explore").await; // no units: nothing counted, nothing logged
    let b = slow_list(&app, 2).await;
    let q = b["queries"].as_array().unwrap();
    assert_eq!(q.len(), 2, "{b}");
    assert_eq!(q[0]["units"], json!([{ "id": "TFT16_U02", "name": "U02" }, { "id": "TFT16_U03", "name": "U03" }]));
    assert_eq!(q[0]["traits"], json!([{ "id": "TFT16_T01", "name": "T01" }]));
    assert_eq!((q[0]["patch"].as_str(), q[0]["bucket"].as_str(), q[0]["window"].as_str()), (Some("16.1"), Some("ALL"), Some("")));
    assert_eq!((q[1]["patch"].as_str(), q[1]["bucket"].as_str(), q[1]["window"].as_str()), (Some("16.2"), Some("MASTER+"), Some("3d")));
    assert_eq!(q[1]["games"], a["summary"]["games"]);
    for e in q {
        assert_eq!(e["source"], json!("index"), "answered from the index");
        assert!(e["ms"].as_u64().is_some() && e["at"].as_i64().unwrap() > 0, "{e}");
    }
    let ix = b["indexes"].as_array().unwrap();
    assert_eq!(ix.len(), 2, "{b}");
    assert!(ix.iter().all(|i| i["state"] == json!("ready") && i["boards"].as_u64().unwrap() > 0 && i["bytes"].as_u64().unwrap() > 0));
    // Slow questions are not crawl problems.
    assert_eq!(get(&app, "/api/health").await["lastProblem"], Value::Null);
    let _ = std::fs::remove_dir_all(dir);

    // The usual threshold: the same questions on this small database are not slow.
    let (app, dir) = make_app("explore-not-slow", routes::SLOW_QUERY_MS, false);
    get(&app, "/api/explore?units=TFT16_U01").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let b = get(&app, "/api/slow-queries").await;
    assert_eq!((b["queries"].clone(), b["thresholdMs"].clone()), (json!([]), json!(500)));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_stress_switch_covers_slow_questions() {
    let (app, dir) = make_app("explore-slow-stress", 0, true);
    get(&app, "/api/explore?units=TFT16_U01").await;
    slow_list(&app, 1).await;
    let worst = get(&app, "/api/slow-queries?data=worst").await;
    let w = worst["queries"].as_array().unwrap();
    assert_eq!(w.len(), 50);
    assert!(w.iter().all(|e| e["units"].as_array().unwrap().len() == 4 && e["traits"].as_array().unwrap().len() == 3));
    assert!(worst["indexes"].as_array().unwrap().iter().any(|i| i["state"] == json!("failed") && i["problem"].as_str().unwrap().len() > 100));
    let empty = get(&app, "/api/slow-queries?data=empty").await;
    assert_eq!((empty["queries"].clone(), empty["indexes"].clone()), (json!([]), json!([])));
    let one = get(&app, "/api/slow-queries?data=one").await;
    assert_eq!((one["queries"].as_array().unwrap().len(), one["indexes"].as_array().unwrap().len()), (1, 1));
    let many = get(&app, "/api/slow-queries?data=many").await;
    assert_eq!(many["queries"].as_array().unwrap().len(), 50);
    // Without the switch (or with an unknown mode) the real list comes back.
    let plain = get(&app, "/api/slow-queries?data=bogus").await;
    assert_eq!(plain["queries"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn malformed_explorer_requests_never_break_the_server() {
    let (app, dir) = make_app("explore-http-fuzz", 0, true);
    let mut r = Rng::new(77);
    let pieces = [
        "units=", "traits=", "patch=", "bucket=", "window=", "data=worst", "data=", "&", "=", ",", "%", "%2", "%ZZ", "%00", "%C3%28",
        "TFT16_U01", "TFT16_T01", "16.2", "+", "%2B", "🙂", "a|b", "&&", "units", "units=TFT16_U01,TFT16_U01,TFT16_U01,TFT16_U01,TFT16_U01",
    ];
    for n in 0..400 {
        let mut q = String::new();
        for _ in 0..r.below(12) {
            q.push_str(pieces[r.below(pieces.len() as u64) as usize]);
        }
        let mut body = vec![0u8; r.below(64) as usize];
        for b in &mut body {
            *b = r.below(256) as u8;
        }
        if n % 3 == 0 {
            body = br#"{"units":["TFT16_U01"],"where":[{"unit":7}],"group":null}"#.to_vec();
        }
        let method = [Method::GET, Method::POST, Method::HEAD, Method::PUT][r.below(4) as usize].clone();
        let path = ["/api/explore", "/api/explore/", "/api/explore/x", "/api/slow-queries"][r.below(4) as usize];
        // Query strings are sent as they are; only bytes a URI cannot hold are escaped.
        let uri: String = format!("{path}?{q}")
            .bytes()
            .map(|b| if b.is_ascii_graphic() { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let (status, json) = call(&app, method.clone(), &uri, body).await;
        assert!(status == 200 || status == 404, "{method} {uri}: {status}");
        if status == 200 && method == Method::GET {
            assert!(json.is_object(), "{method} {uri}: not a JSON object");
        }
    }
    // Still answering afterwards.
    assert!(get(&app, "/api/explore?units=TFT16_U01").await["summary"]["games"].as_i64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(dir);
}
