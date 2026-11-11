//! The Data page's engine routes, which only the Rust API has (there is no TypeScript twin to match):
//!
//! - `GET /api/health`: matches stored per hour per server, matches waiting per server, the last problem, how old
//!   the stats are (E10), and how often a match's stored rank puts its lobby in the right bucket (E7).
//! - `GET /api/settings/crawl`: which servers and tiers the crawler uses, and the choices the app offers (E9).
//! - `POST /api/settings/crawl`: saves new choices; a refused change is a 400 with the reason in plain words.
//! - `POST /api/settings/crawl/reset`: forgets the saved choices, so `PLATFORMS` and `TIERS` from `.env` apply.
//!
//! The crawler reads the settings at the top of every round, so a saved change applies on the next round.
//!
//! In debug builds (`cargo build`, which is what development uses) the two GET routes also take the dev stress
//! switch `?data=worst|empty|one|many`, like the TypeScript server's `--dev` mode. Release builds ignore it.

use crate::routes::Obj;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};
use tft_core::config::{BUCKETS, Config, PLATFORMS, TIERS, platform_route};
use tft_store::{CRAWL_SETTINGS, CrawlSettings, SettingsError};

/// Riot's servers as the client names them (the same names as `SERVERS` in `src/ui/Account.tsx`).
pub const SERVER_LABELS: [(&str, &str); 15] = [
    ("na1", "North America"),
    ("la1", "Latin America North"),
    ("la2", "Latin America South"),
    ("br1", "Brazil"),
    ("euw1", "Europe West"),
    ("eun1", "Europe Nordic & East"),
    ("tr1", "Türkiye"),
    ("ru", "Russia"),
    ("me1", "Middle East"),
    ("kr", "Korea"),
    ("jp1", "Japan"),
    ("oc1", "Oceania"),
    ("sg2", "Southeast Asia"),
    ("tw2", "Taiwan"),
    ("vn2", "Vietnam"),
];

fn server_label(p: &str) -> String {
    SERVER_LABELS.iter().find(|(v, _)| *v == p).map_or_else(|| p.to_string(), |(_, l)| l.to_string())
}

fn tier_label(t: &str) -> String {
    let lower = t.to_lowercase();
    let mut c = lower.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

fn obj(v: Value) -> Obj {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("always built from json!({{..}})"),
    }
}

/// `GET /api/health`. Every server being crawled gets a row, plus any other server that stored a match in the
/// last 24 hours or still has matches waiting.
pub fn health(conn: &Connection, config: &Config, query: Option<&str>) -> Result<Obj> {
    let h = tft_store::health(conn)?;
    let acc = tft_store::rank_sample_accuracy(conn)?;
    let crawling = tft_store::get_crawl_settings(conn, config)?.platforms;
    let mut names: Vec<String> = crawling.clone();
    for r in &h.per_region {
        if !names.contains(&r.region) {
            names.push(r.region.clone());
        }
    }
    let regions: Vec<Value> = names
        .iter()
        .map(|name| {
            let r = h.per_region.iter().find(|r| &r.region == name);
            let queued = h.queue_depth_per_region.iter().find(|q| &q.region == name).map_or(0, |q| q.pending);
            json!({
                "region": name,
                "label": server_label(name),
                "crawling": crawling.contains(name),
                "matchesLastHour": r.map_or(0, |r| r.matches_last_hour),
                "last24h": r.map_or_else(|| vec![0; 24], |r| r.matches_last_24h_per_hour.clone()),
                "queued": queued,
            })
        })
        .collect();
    let body = json!({
        "regions": regions,
        "lastProblem": h.last_error.map(|e| json!({
            "at": e.at,
            "region": e.region,
            "level": e.level,
            "message": e.message,
        })),
        "statsBuiltAt": h.stats_built_at,
        "statsAgeMinutes": h.stats_age_minutes,
        "newestMatchAt": h.newest_match_at,
        "rankAccuracy": {
            "sampledMatches": acc.sampled_matches,
            "players": acc.players,
            "share": acc.share_in_right_bucket,
            "sampleRate": config.rank_sample_rate,
            "perBucket": acc.per_bucket.iter().map(|b| json!({
                "bucket": b.bucket,
                "sampledMatches": b.sampled_matches,
                "inRightBucket": b.in_right_bucket,
                "real": b.real.iter().map(|(k, n)| json!({ "bucket": k, "matches": n })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        },
    });
    Ok(stress_health(stress_mode(query).as_deref(), obj(body)))
}

/// The settings as the app shows them: what the crawler uses now, whether that was chosen in the app, the
/// `.env` defaults, and every server and tier that can be chosen.
fn settings_body(conn: &Connection, config: &Config) -> Result<Obj> {
    let current = tft_store::get_crawl_settings(conn, config)?;
    let saved =
        tft_store::kv_get::<CrawlSettings>(conn, CRAWL_SETTINGS).ok().flatten().is_some_and(|s| s.validated().is_ok());
    let defaults = CrawlSettings::from_config(config);
    // PLATFORMS is every server the crawler can route; the labels table follows the client's order.
    let servers: Vec<Value> = SERVER_LABELS
        .iter()
        .filter(|(v, _)| PLATFORMS.contains(v))
        .map(|(v, l)| json!({ "value": v, "label": l, "route": platform_route(v) }))
        .collect();
    let tiers: Vec<Value> = TIERS.iter().rev().map(|t| json!({ "value": t, "label": tier_label(t) })).collect();
    Ok(obj(json!({
        "platforms": current.platforms,
        "tiers": current.tiers,
        "saved": saved,
        "defaults": { "platforms": defaults.platforms, "tiers": defaults.tiers },
        "serverOptions": servers,
        "tierOptions": tiers,
    })))
}

/// `GET /api/settings/crawl`.
pub fn get_settings(conn: &Connection, config: &Config, query: Option<&str>) -> Result<Obj> {
    let body = settings_body(conn, config)?;
    Ok(stress_settings(stress_mode(query).as_deref(), body))
}

fn bad_request(message: impl std::fmt::Display) -> (u16, Obj) {
    (400, obj(json!({ "error": message.to_string() })))
}

/// `POST /api/settings/crawl` with `{ platforms: string[], tiers: string[] }`. Returns the status and body:
/// 200 with the saved settings (as `GET` shows them), or 400 with `{ error }` and nothing saved.
pub fn set_settings(conn: &Connection, config: &Config, body: &[u8]) -> Result<(u16, Obj)> {
    let Ok(Value::Object(b)) = serde_json::from_slice::<Value>(body) else {
        return Ok(bad_request("Send the settings as JSON: { \"platforms\": [...], \"tiers\": [...] }."));
    };
    let list = |key: &str| -> Option<Vec<String>> {
        match b.get(key) {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(xs)) => xs.iter().map(|x| x.as_str().map(str::to_string)).collect(),
            Some(_) => None,
        }
    };
    let (Some(platforms), Some(tiers)) = (list("platforms"), list("tiers")) else {
        return Ok(bad_request("Servers and tiers must each be a list of names."));
    };
    match tft_store::set_crawl_settings(conn, &CrawlSettings { platforms, tiers }) {
        Ok(_) => Ok((200, settings_body(conn, config)?)),
        Err(e) => match e.downcast_ref::<SettingsError>() {
            Some(refused) => Ok(bad_request(refused)),
            None => Err(e),
        },
    }
}

/// `POST /api/settings/crawl/reset`.
pub fn reset_settings(conn: &Connection, config: &Config) -> Result<Obj> {
    tft_store::reset_crawl_settings(conn)?;
    settings_body(conn, config)
}

// ---- Dev stress switch (debug builds only) ---------------------------------------------------------------

fn stress_mode(query: Option<&str>) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    query?.split('&').find_map(|kv| kv.strip_prefix("data=")).map(str::to_string).filter(|m| !m.is_empty())
}

const HOUR: i64 = 3_600_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn fake_region(name: &str, crawling: bool, last24h: Vec<i64>, queued: i64) -> Value {
    json!({
        "region": name, "label": server_label(name), "crawling": crawling,
        "matchesLastHour": last24h[23], "last24h": last24h, "queued": queued,
    })
}

fn fake_accuracy(per: &[(i64, [i64; 4])], rate: f64) -> Value {
    let total: i64 = per.iter().map(|(_, r)| r.iter().sum::<i64>()).sum();
    let right: i64 = per.iter().enumerate().map(|(i, (_, r))| r[i]).sum();
    json!({
        "sampledMatches": total,
        "players": total * 8,
        "share": (total > 0).then(|| right as f64 / total as f64),
        "sampleRate": rate,
        "perBucket": BUCKETS.iter().zip(per).map(|(b, (_, real))| json!({
            "bucket": b.key,
            "sampledMatches": real.iter().sum::<i64>(),
            "inRightBucket": real[BUCKETS.iter().position(|x| x.key == b.key).unwrap()],
            "real": BUCKETS.iter().zip(real).map(|(k, n)| json!({ "bucket": k.key, "matches": n })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Reshapes a health body into the stress datasets the UI is checked against.
fn stress_health(mode: Option<&str>, mut body: Obj) -> Obj {
    let now = now_ms();
    let zero = [(0, [0; 4]); 4];
    match mode {
        Some("empty") => {
            body.insert("regions".into(), json!([]));
            body.insert("lastProblem".into(), Value::Null);
            body.insert("statsBuiltAt".into(), Value::Null);
            body.insert("statsAgeMinutes".into(), Value::Null);
            body.insert("newestMatchAt".into(), Value::Null);
            body.insert("rankAccuracy".into(), fake_accuracy(&zero, 0.0));
        }
        Some("one") => {
            let mut h = vec![0; 24];
            h[23] = 1;
            body.insert("regions".into(), json!([fake_region("na1", true, h, 1)]));
            body.insert("lastProblem".into(), Value::Null);
            body.insert("statsBuiltAt".into(), json!(now - 60_000));
            body.insert("statsAgeMinutes".into(), json!(1));
            body.insert("newestMatchAt".into(), json!(now - 60_000));
            body.insert(
                "rankAccuracy".into(),
                fake_accuracy(&[(0, [0; 4]), (0, [0; 4]), (0, [0, 0, 1, 0]), (0, [0; 4])], 0.02),
            );
        }
        Some("worst") => {
            // Every server, numbers far larger than a real crawl, one hour with a spike, and a long error.
            let regions: Vec<Value> = PLATFORMS
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let mut h: Vec<i64> = (0..24).map(|k| if (k + i) % 7 == 0 { 0 } else { 9_999 }).collect();
                    h[(i * 5) % 24] = 1_234_567;
                    fake_region(p, i % 3 != 2, h, 987_654_321 / (i as i64 + 1))
                })
                .collect();
            body.insert("regions".into(), json!(regions));
            body.insert(
                "lastProblem".into(),
                json!({
                    "at": now - 400 * 24 * HOUR,
                    "region": "americas",
                    "level": "error",
                    "message": "Riot answered 503 Service Unavailable for https://americas.api.riotgames.com/tft/match/v1/matches/NA1_5123456789012345?with=a_very_long_query_string_that_does_not_break_anywhere_on_its_own_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and kept doing so after five retries, so this region is paused for ten minutes before it tries again.",
                }),
            );
            body.insert("statsBuiltAt".into(), json!(now - 400 * 24 * HOUR));
            body.insert("statsAgeMinutes".into(), json!(400 * 24 * 60));
            body.insert("newestMatchAt".into(), json!(now - 3 * 24 * HOUR));
            body.insert(
                "rankAccuracy".into(),
                fake_accuracy(&[(0, [1, 0, 0, 999_999]), (0, [0; 4]), (0, [123_456, 1, 0, 0]), (0, [0, 0, 1, 0])], 1.0),
            );
        }
        Some("many") => {
            let regions: Vec<Value> = PLATFORMS
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let h: Vec<i64> = (0..24).map(|k| ((k * 37 + i as i64 * 11) % 90) * (i as i64 + 1)).collect();
                    fake_region(p, true, h, (i as i64 * 131) % 700)
                })
                .collect();
            body.insert("regions".into(), json!(regions));
            body.insert(
                "lastProblem".into(),
                json!({ "at": now - 12 * 60_000, "region": "europe", "level": "warn", "message": "Riot asked us to slow down. Waiting 20 seconds." }),
            );
            body.insert("statsBuiltAt".into(), json!(now - 35 * 60_000));
            body.insert("statsAgeMinutes".into(), json!(35));
            body.insert("newestMatchAt".into(), json!(now - 30_000));
            body.insert(
                "rankAccuracy".into(),
                fake_accuracy(
                    &[(0, [612, 140, 9, 0]), (0, [96, 581, 118, 4]), (0, [3, 101, 402, 77]), (0, [0, 6, 88, 311])],
                    0.02,
                ),
            );
        }
        _ => {}
    }
    body
}

fn stress_settings(mode: Option<&str>, mut body: Obj) -> Obj {
    match mode {
        // Everything chosen: the longest the chips can get.
        Some("worst") | Some("many") => {
            body.insert("platforms".into(), json!(PLATFORMS));
            body.insert("tiers".into(), json!(TIERS));
            body.insert("saved".into(), json!(true));
        }
        Some("one") => {
            body.insert("platforms".into(), json!(["na1"]));
            body.insert("tiers".into(), json!(["CHALLENGER"]));
            body.insert("saved".into(), json!(true));
        }
        _ => {}
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, Pool, handle};
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Method, Request};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tft_store::params;

    fn config() -> Config {
        Config::from_lookup(|k| match k {
            "PLATFORMS" => Some("na1,euw1".into()),
            "TIERS" => Some("CHALLENGER,MASTER".into()),
            "RANK_SAMPLE_RATE" => Some("0.02".into()),
            _ => None,
        })
    }

    /// A fresh database file and the whole app around it, as `serve` builds it.
    fn app(name: &str) -> (Arc<App>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("tft-api-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("tft.db");
        let write = tft_store::open(&db).unwrap();
        let calendar = tft_store::load_calendar(&write).unwrap();
        let write = Arc::new(Mutex::new(write));
        let config = config();
        let me = Arc::new(crate::me::Me::new(Arc::clone(&write), &config, calendar));
        let app = Arc::new(App {
            pool: Pool { path: db.clone(), idle: Mutex::new(Vec::new()) },
            comps: Mutex::default(),
            icons: dir.join("icons"),
            dist: None,
            http: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(1)).build().unwrap(),
            write,
            me,
            config,
        });
        (app, db)
    }

    async fn call(app: &Arc<App>, method: Method, uri: &str, body: &str) -> (u16, Value) {
        let req = Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
        let res = handle(State(Arc::clone(app)), req).await;
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn every_known_server_has_a_label() {
        for p in PLATFORMS {
            assert!(SERVER_LABELS.iter().any(|(v, _)| *v == p), "{p} has no label");
        }
        assert_eq!(tier_label("GRANDMASTER"), "Grandmaster");
    }

    #[tokio::test]
    async fn health_on_an_empty_database_lists_the_servers_being_crawled() {
        let (app, _) = app("empty");
        let (status, h) = call(&app, Method::GET, "/api/health", "").await;
        assert_eq!(status, 200);
        let regions = h["regions"].as_array().unwrap();
        assert_eq!(regions.iter().map(|r| r["region"].as_str().unwrap()).collect::<Vec<_>>(), ["na1", "euw1"]);
        assert_eq!(regions[0]["label"], "North America");
        assert_eq!(regions[0]["crawling"], true);
        assert_eq!(regions[0]["matchesLastHour"], 0);
        assert_eq!(regions[0]["last24h"], json!(vec![0; 24]));
        assert_eq!(regions[0]["queued"], 0);
        assert_eq!(h["lastProblem"], Value::Null);
        assert_eq!(h["statsBuiltAt"], Value::Null);
        assert_eq!(h["statsAgeMinutes"], Value::Null);
        assert_eq!(h["newestMatchAt"], Value::Null);
        let acc = &h["rankAccuracy"];
        assert_eq!(acc["sampledMatches"], 0);
        assert_eq!(acc["share"], Value::Null, "no share until a sample is finished");
        assert_eq!(acc["sampleRate"], 0.02);
        assert_eq!(acc["perBucket"].as_array().unwrap().len(), BUCKETS.len());
    }

    #[tokio::test]
    async fn health_reports_matches_queue_problems_freshness_and_rank_accuracy() {
        let (app, _) = app("full");
        let now = now_ms();
        {
            let c = app.write.lock().unwrap();
            for (id, p, ago) in
                [("N1", "na1", 60_000), ("N2", "na1", 120_000), ("N3", "na1", 3 * HOUR + 1), ("K1", "kr", 600_000)]
            {
                c.execute(
                    "INSERT INTO match_stored (match_id, platform, at) VALUES (?1, ?2, ?3)",
                    params![id, p, now - ago],
                )
                .unwrap();
            }
            for (id, route, p) in [("Q1", "europe", "euw1"), ("Q2", "europe", "euw1"), ("Q3", "americas", "na1")] {
                c.execute(
                    "INSERT INTO match_queue (match_id, route, platform, seed_rank, status) VALUES (?1, ?2, ?3, 9, 0)",
                    params![id, route, p],
                )
                .unwrap();
            }
            c.execute(
                "INSERT INTO stat_meta (patch, bucket, matches, comps, built_at) VALUES ('16.1', 'ALL', 1, 1, ?1)",
                [now - 7 * 60_000 - 1_000],
            )
            .unwrap();
            tft_store::log_event_at(&c, now - 5_000, "europe", "warn", "Riot asked us to slow down.").unwrap();
            // Two finished samples: one Master lobby stored as Master (right), one Diamond lobby stored as Master.
            for (id, real) in [("S1", "MASTER"), ("S2", "DIAMOND")] {
                c.execute(
                    "INSERT INTO matches (match_id, platform, patch, game_datetime, rank) VALUES (?1, 'na1', '16.1', 0, 7)",
                    [id],
                )
                .unwrap();
                tft_store::queue_sample(&c, id, "na1").unwrap();
                for i in 0..8 {
                    let puuid = format!("{id}-{i}");
                    c.execute(
                        "INSERT INTO participants (match_id, puuid, placement) VALUES (?1, ?2, ?3)",
                        params![id, puuid, i + 1],
                    )
                    .unwrap();
                    tft_store::save_rank(&c, id, &puuid, real, "I").unwrap();
                }
                assert!(tft_store::complete_sample(&c, id).unwrap());
            }
        }
        let (status, h) = call(&app, Method::GET, "/api/health", "").await;
        assert_eq!(status, 200);
        let regions = h["regions"].as_array().unwrap();
        let names: Vec<&str> = regions.iter().map(|r| r["region"].as_str().unwrap()).collect();
        assert_eq!(names, ["na1", "euw1", "kr"], "crawled servers first, then others with activity");
        let na1 = &regions[0];
        assert_eq!(na1["matchesLastHour"], 2);
        assert_eq!(na1["last24h"][23], 2);
        assert_eq!(na1["last24h"][20], 1, "three hours ago");
        assert_eq!(na1["queued"], 1);
        assert_eq!(regions[1]["queued"], 2);
        assert_eq!(regions[1]["matchesLastHour"], 0);
        assert_eq!(regions[2]["crawling"], false);
        assert_eq!(regions[2]["label"], "Korea");
        assert_eq!(h["lastProblem"]["message"], "Riot asked us to slow down.");
        assert_eq!(h["lastProblem"]["region"], "europe");
        assert_eq!(h["lastProblem"]["level"], "warn");
        assert_eq!(h["statsAgeMinutes"], 7);
        assert!(h["newestMatchAt"].as_i64().unwrap() >= now - 60_000);
        let acc = &h["rankAccuracy"];
        assert_eq!(acc["sampledMatches"], 2);
        assert_eq!(acc["players"], 16);
        assert_eq!(acc["share"], 0.5);
        let master = acc["perBucket"].as_array().unwrap().iter().find(|b| b["bucket"] == "MASTER+").unwrap();
        assert_eq!(master["sampledMatches"], 2);
        assert_eq!(master["inRightBucket"], 1);
        assert_eq!(master["real"][2], json!({ "bucket": "DIAMOND+", "matches": 1 }));
    }

    #[tokio::test]
    async fn settings_start_from_env_save_and_reset() {
        let (app, _) = app("settings");
        let (status, s) = call(&app, Method::GET, "/api/settings/crawl", "").await;
        assert_eq!(status, 200);
        assert_eq!(strs(&s["platforms"]), ["na1", "euw1"]);
        assert_eq!(strs(&s["tiers"]), ["CHALLENGER", "MASTER"]);
        assert_eq!(s["saved"], false);
        assert_eq!(strs(&s["defaults"]["platforms"]), ["na1", "euw1"]);
        assert_eq!(s["serverOptions"].as_array().unwrap().len(), PLATFORMS.len());
        assert_eq!(s["serverOptions"][0], json!({ "value": "na1", "label": "North America", "route": "americas" }));
        assert_eq!(s["tierOptions"][0], json!({ "value": "CHALLENGER", "label": "Challenger" }));
        assert_eq!(s["tierOptions"].as_array().unwrap().len(), TIERS.len());

        let (status, s) =
            call(&app, Method::POST, "/api/settings/crawl", r#"{"platforms":[" KR ","oc1","kr"],"tiers":["diamond"]}"#)
                .await;
        assert_eq!(status, 200);
        assert_eq!(strs(&s["platforms"]), ["kr", "oc1"], "saved as cleaned up");
        assert_eq!(strs(&s["tiers"]), ["DIAMOND"]);
        assert_eq!(s["saved"], true);
        // What the crawler reads at the top of its next round.
        let read = tft_store::get_crawl_settings(&app.write.lock().unwrap(), &app.config).unwrap();
        assert_eq!(read.platforms, ["kr", "oc1"]);
        let (_, again) = call(&app, Method::GET, "/api/settings/crawl", "").await;
        assert_eq!(strs(&again["platforms"]), ["kr", "oc1"]);
        let (_, h) = call(&app, Method::GET, "/api/health", "").await;
        let names: Vec<&str> = h["regions"].as_array().unwrap().iter().map(|r| r["region"].as_str().unwrap()).collect();
        assert_eq!(names, ["kr", "oc1"], "health follows the saved servers");

        let (status, s) = call(&app, Method::POST, "/api/settings/crawl/reset", "").await;
        assert_eq!(status, 200);
        assert_eq!(strs(&s["platforms"]), ["na1", "euw1"]);
        assert_eq!(s["saved"], false);
    }

    #[tokio::test]
    async fn bad_settings_are_a_400_with_the_reason_and_nothing_is_saved() {
        let (app, _) = app("refused");
        for (body, error) in [
            (r#"{"platforms":[],"tiers":["MASTER"]}"#, "Choose at least one server to crawl."),
            (r#"{"tiers":["MASTER"]}"#, "Choose at least one server to crawl."),
            (r#"{"platforms":["na1"],"tiers":[]}"#, "Choose at least one tier to collect players from."),
            (r#"{"platforms":["na1","mars1"],"tiers":["MASTER"]}"#, "Unknown server(s): mars1."),
            (r#"{"platforms":["na1"],"tiers":["WOOD"]}"#, "Unknown tier(s): WOOD."),
            (r#"{"platforms":"na1","tiers":["MASTER"]}"#, "Servers and tiers must each be a list of names."),
            (r#"{"platforms":["na1",3],"tiers":["MASTER"]}"#, "Servers and tiers must each be a list of names."),
            ("not json", "Send the settings as JSON: { \"platforms\": [...], \"tiers\": [...] }."),
            ("[]", "Send the settings as JSON: { \"platforms\": [...], \"tiers\": [...] }."),
        ] {
            let (status, b) = call(&app, Method::POST, "/api/settings/crawl", body).await;
            assert_eq!(status, 400, "{body}");
            assert_eq!(b, json!({ "error": error }), "{body}");
        }
        let (_, s) = call(&app, Method::GET, "/api/settings/crawl", "").await;
        assert_eq!(s["saved"], false);
        assert_eq!(strs(&s["platforms"]), ["na1", "euw1"]);
    }

    #[tokio::test]
    async fn only_the_right_methods_reach_the_settings_routes() {
        let (app, _) = app("methods");
        // No built app here, so anything unmatched is the plain 404.
        let (status, _) = call(&app, Method::DELETE, "/api/settings/crawl", "").await;
        assert_eq!(status, 404);
        let (status, _) = call(&app, Method::GET, "/api/settings/crawl/reset", "").await;
        assert_eq!(status, 404);
        let (status, _) = call(&app, Method::POST, "/api/health", "{}").await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn the_stress_switch_reshapes_the_get_routes_in_debug_builds() {
        let (app, _) = app("stress");
        let (_, h) = call(&app, Method::GET, "/api/health?data=worst", "").await;
        assert_eq!(h["regions"].as_array().unwrap().len(), PLATFORMS.len());
        assert!(h["lastProblem"]["message"].as_str().unwrap().len() > 200);
        let (_, h) = call(&app, Method::GET, "/api/health?data=empty", "").await;
        assert_eq!(h["regions"], json!([]));
        assert_eq!(h["rankAccuracy"]["share"], Value::Null);
        let (_, h) = call(&app, Method::GET, "/api/health?data=one", "").await;
        assert_eq!(h["rankAccuracy"]["sampledMatches"], 1);
        assert_eq!(h["rankAccuracy"]["share"], 1.0);
        let (_, h) = call(&app, Method::GET, "/api/health?data=many", "").await;
        let acc = &h["rankAccuracy"];
        let sum: i64 = acc["perBucket"].as_array().unwrap().iter().map(|b| b["sampledMatches"].as_i64().unwrap()).sum();
        assert_eq!(acc["sampledMatches"].as_i64().unwrap(), sum);
        let (_, s) = call(&app, Method::GET, "/api/settings/crawl?data=worst", "").await;
        assert_eq!(s["platforms"].as_array().unwrap().len(), PLATFORMS.len());
        assert_eq!(stress_mode(Some("x=1&data=")), None);
    }
}
