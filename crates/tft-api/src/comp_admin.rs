//! The comp routes of PRD 2 that are not in the TypeScript API:
//!
//! - `POST /api/comps/:id/rename` `{ name }`: your name for a comp; an empty name goes back to the rule's (C14).
//! - `POST /api/comps/:id/merge` `{ into }`: count this comp's boards as the other comp's from the next stats
//!   build on; `into` empty or null undoes it (C14).
//! - `DELETE /api/comps/:id/override`: forget your rename and merge of a comp.
//! - `GET /api/comps/:id/override`: your rename and merge of a comp, or null.
//! - `GET /api/comps/:id/variants?patch=&bucket=&window=`: the comp's variants (C2), clusters method only.
//! - `GET /api/settings/comps`, `POST /api/settings/comps` `{ method }`: how boards are grouped into comps,
//!   `baseline` (today's rule, the default) or `clusters` (C1).
//!
//! Also the naming hooks the comp list uses: your renames, and the trait and carry of a comp whose id is a
//! cluster id rather than "trait|carry".

use crate::describe::{Kind, describe};
use crate::js::num;
use crate::routes::{Obj, scope};
use crate::{App, json, json_status, server_error};
use anyhow::Result;
use axum::body::Body;
use axum::http::Method;
use axum::response::Response;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;
use tft_stats::overrides;

pub(crate) enum AdminRoute {
    Rename(String),
    Merge(String),
    Clear(String),
    Override(String),
    Variants(String),
    GetMethod,
    SetMethod,
}

pub(crate) fn route(method: &Method, path: &str) -> Option<AdminRoute> {
    let seg: Vec<&str> = path.split('/').collect();
    let id = |s: &str| crate::js::try_decode_component(s);
    let (get, post, delete) = (method == Method::GET, method == Method::POST, method == Method::DELETE);
    Some(match seg.as_slice() {
        ["", "api", "comps", c, "rename"] if post && !c.is_empty() => AdminRoute::Rename(id(c)),
        ["", "api", "comps", c, "merge"] if post && !c.is_empty() => AdminRoute::Merge(id(c)),
        ["", "api", "comps", c, "override"] if delete && !c.is_empty() => AdminRoute::Clear(id(c)),
        ["", "api", "comps", c, "override"] if get && !c.is_empty() => AdminRoute::Override(id(c)),
        ["", "api", "comps", c, "variants"] if get && !c.is_empty() => AdminRoute::Variants(id(c)),
        ["", "api", "settings", "comps"] if get => AdminRoute::GetMethod,
        ["", "api", "settings", "comps"] if post => AdminRoute::SetMethod,
        _ => return None,
    })
}

fn obj(v: Value) -> Obj {
    match v {
        Value::Object(o) => o,
        _ => unreachable!(),
    }
}

fn override_json(o: Option<overrides::Override>) -> Value {
    match o {
        Some(o) => json!({ "id": o.comp_id, "name": o.name, "mergeInto": o.merge_into }),
        None => Value::Null,
    }
}

fn bad_request(message: impl std::fmt::Display) -> (u16, Obj) {
    (400, obj(json!({ "error": message.to_string() })))
}

/// A string field of a JSON object body; `None` when the body is not an object.
fn field(body: &[u8], key: &str) -> Option<Option<String>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(b)) => Some(match b.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// Answers one of the [`AdminRoute`]s.
pub(crate) async fn handle(app: &Arc<App>, r: AdminRoute, body: Body, query: Option<String>) -> Response {
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return server_error(e.into()),
    };
    let writes = !matches!(r, AdminRoute::Override(_) | AdminRoute::Variants(_) | AdminRoute::GetMethod);
    let app2 = Arc::clone(app);
    let done = tokio::task::spawn_blocking(move || -> Result<(u16, Obj)> {
        if writes {
            let db = app2.write.lock().unwrap();
            let out = write(&db, r, &bytes);
            app2.comps.lock().unwrap().clear(); // names are part of the cached comp list
            out
        } else {
            let conn = app2.pool.take()?;
            let out = read(&conn, r, query.as_deref());
            app2.pool.give(conn);
            out
        }
    })
    .await;
    match done {
        Ok(Ok((200, body))) => json(body),
        Ok(Ok((status, body))) => json_status(status, body),
        Ok(Err(e)) => server_error(e),
        Err(e) => server_error(e.into()),
    }
}

fn write(db: &Connection, r: AdminRoute, body: &[u8]) -> Result<(u16, Obj)> {
    Ok(match r {
        AdminRoute::Rename(id) => {
            let Some(name) = field(body, "name") else { return Ok(bad_request("Send the new name as { \"name\": \"...\" }.")) };
            let o = overrides::rename(db, &id, name.as_deref().unwrap_or(""))?;
            let note = if o.name.is_some() { "Renamed." } else { "Back to the name the comp rules give." };
            (200, obj(json!({ "override": override_json(overrides::get(db, &id)?), "note": note })))
        }
        AdminRoute::Merge(id) => {
            let Some(into) = field(body, "into") else {
                return Ok(bad_request("Send the comp to merge into as { \"into\": \"comp id\" }."));
            };
            match overrides::merge(db, &id, into.as_deref()) {
                Ok(o) => {
                    let note = if o.merge_into.is_some() {
                        "Merged. The comp list changes at the next stats build (within 10 minutes while collecting, or run tft stats)."
                    } else {
                        "Merge undone. The comp comes back at the next stats build."
                    };
                    (200, obj(json!({ "override": override_json(overrides::get(db, &id)?), "note": note })))
                }
                Err(e) => bad_request(e),
            }
        }
        AdminRoute::Clear(id) => {
            overrides::clear(db, &id)?;
            (200, obj(json!({ "override": null, "note": "Your name and merge for this comp are gone." })))
        }
        AdminRoute::SetMethod => {
            let Some(Some(m)) = field(body, "method") else {
                return Ok(bad_request("Send the method as { \"method\": \"baseline\" } or { \"method\": \"clusters\" }."));
            };
            let Some(m) = tft_stats::Method::parse(&m) else { return Ok(bad_request("Choose baseline or clusters.")) };
            let changed = tft_stats::set_comp_method(db, m)?;
            let mut out = method_body(db)?;
            out.insert("changed".into(), json!(changed));
            (200, out)
        }
        _ => unreachable!("read routes go to read()"),
    })
}

fn method_body(db: &Connection) -> Result<Obj> {
    Ok(obj(json!({
        "method": tft_stats::comp_method(db)?.as_str(),
        "options": [
            { "value": "baseline", "label": "Biggest trait and main carry (today's rule)" },
            { "value": "clusters", "label": "Units the boards share (new)" },
        ],
        "note": "A change takes effect at the next stats build.",
    })))
}

fn read(conn: &Connection, r: AdminRoute, q: Option<&str>) -> Result<(u16, Obj)> {
    Ok(match r {
        AdminRoute::Override(id) => (200, obj(json!({ "override": override_json(overrides::get(conn, &id)?) }))),
        AdminRoute::GetMethod => (200, method_body(conn)?),
        AdminRoute::Variants(id) => (200, variants(conn, q, &id)?),
        _ => unreachable!("write routes go to write()"),
    })
}

/// The comp's variants in a scope, most games first, with their rates and pick rate (boards per lobby).
fn variants(conn: &Connection, q: Option<&str>, id: &str) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let mut st = conn.prepare(
        "SELECT variant_id, units, carries, games, sum_place, wins, top4 FROM stat_comp_variants
         WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY games DESC, variant_id",
    )?;
    let rows: Vec<(String, String, String, i64, i64, i64, i64)> = st
        .query_map(rusqlite::params![sc.patch, sc.key, id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (vid, units, carries, games, sum_place, wins, top4) in rows {
        let list = |ids: &str| -> Result<Vec<Value>> {
            tft_stats::cluster::parse_list(ids).iter().map(|u| Ok(Value::Object(describe(conn, Kind::Unit, u, false)?))).collect()
        };
        let g = games as f64;
        out.push(json!({
            "id": vid,
            "units": list(&units)?,
            "carries": list(&carries)?,
            "games": games,
            "avgPlace": num(sum_place as f64 / g),
            "winRate": num(wins as f64 / g),
            "top4Rate": num(top4 as f64 / g),
            "pickRate": if sc.matches != 0 { num(g / sc.matches as f64) } else { json!(0) },
        }));
    }
    Ok(obj(json!({ "patch": sc.patch, "bucket": sc.bucket, "window": sc.window, "variants": out })))
}

// ---- Naming hooks for the comp list ------------------------------------------------

/// Your name for a comp, if you renamed it.
pub(crate) fn name_override(conn: &Connection, comp_key: &str) -> Result<Option<String>> {
    // A database opened read-only before the table existed has no comp_overrides yet.
    match overrides::name_of(conn, comp_key) {
        Ok(n) => Ok(n),
        Err(e) if e.to_string().contains("no such table") => Ok(None),
        Err(e) => Err(e),
    }
}

fn num_of(o: &Obj, k: &str) -> f64 {
    o.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}
fn str_of<'a>(o: &'a Obj, k: &str) -> &'a str {
    o.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The trait and main carry a comp is named after. A baseline id is "trait|carry". A cluster id says neither,
/// so (PRD 2 step 6): the trait with the most units among traits active on at least half the boards (at its
/// most common level), and the unit holding three items most often (or the most played unit).
/// `traits` and `units` are the comp's stat rows, most games first.
pub(crate) fn trait_and_carry(comp_key: &str, traits: &[Obj], units: &[Obj], games: f64) -> (String, String) {
    if comp_key.contains('|') {
        let mut split = comp_key.split('|');
        return (split.next().unwrap_or("").to_string(), split.next().unwrap_or("").to_string());
    }
    let mut seen: Vec<(&str, f64, f64)> = Vec::new(); // name, games at any level, units at its most common level
    for t in traits {
        let name = str_of(t, "name");
        match seen.iter_mut().find(|s| s.0 == name) {
            Some(s) => s.1 += num_of(t, "games"),
            None => seen.push((name, num_of(t, "games"), num_of(t, "num_units"))),
        }
    }
    let top = seen
        .into_iter()
        .filter(|s| games > 0.0 && s.1 / games >= 0.5)
        .max_by(|a, b| a.2.total_cmp(&b.2).then(a.1.total_cmp(&b.1)).then(b.0.cmp(a.0)))
        .map_or("Flex".to_string(), |s| s.0.to_string());
    let carry = units
        .iter()
        .max_by(|a, b| {
            num_of(a, "full_items")
                .total_cmp(&num_of(b, "full_items"))
                .then(num_of(a, "games").total_cmp(&num_of(b, "games")))
                .then(str_of(b, "character_id").cmp(str_of(a, "character_id")))
        })
        .map_or(String::new(), |u| str_of(u, "character_id").to_string());
    (top, carry)
}

#[cfg(test)]
mod tests {
    use crate::{App, Pool, handle};
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Method, Request};
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tft_core::config::Config;
    use tft_store::params;

    fn app(name: &str) -> (Arc<App>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("tft-api-comp-admin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("tft.db");
        let write = tft_store::open(&db).unwrap();
        let calendar = tft_store::load_calendar(&write).unwrap();
        let write = Arc::new(Mutex::new(write));
        let config = Config::from_lookup(|_| None);
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
            dev: false,
        });
        (app, dir)
    }

    async fn call(app: &Arc<App>, method: Method, uri: &str, body: &str) -> (u16, Value) {
        let req = Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
        let res = handle(State(Arc::clone(app)), req).await;
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Two comps of 60 boards each (stored labels "T_X|X5" and "T_Y|Y5"); X's carry varies so it has variants.
    fn boards(app: &Arc<App>) {
        let db = app.write.lock().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        for i in 0..240usize {
            let (label, carry) = if i < 160 { ("X", if i < 80 { 4 } else { 5 }) } else { ("Y", 5) };
            let m = format!("M{}", i / 8);
            db.execute("INSERT OR IGNORE INTO matches VALUES (?, 'na1', '91.1', 99, ?, 2000, 8, NULL)", params![m, now - 3_600_000]).unwrap();
            db.execute(
                "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, 8, ?)",
                params![m, format!("P{i}"), 1 + (i % 8) as i64, format!("T_{label}|TFT99_{label}5")],
            )
            .unwrap();
            let id = db.last_insert_rowid();
            for u in 1..=6 {
                let items = if u == carry { 3 } else { 0 };
                let rarity = [0, 0, 1, 2, 4, 6][u - 1];
                db.execute("INSERT INTO units VALUES (?, ?, 2, ?, NULL, ?)", params![id, format!("TFT99_{label}{u}"), items, rarity]).unwrap();
            }
            db.execute("INSERT INTO traits VALUES (?, ?, 6, 2, 2)", params![id, format!("T_{label}")]).unwrap();
        }
    }

    fn rebuild(app: &Arc<App>) {
        tft_stats::build_stats_incremental_with(&mut app.write.lock().unwrap(), 2, &mut |_| {}).unwrap();
    }

    fn names(v: &Value) -> Vec<(String, String)> {
        v["rows"].as_array().unwrap().iter().map(|r| (r["id"].as_str().unwrap().into(), r["name"].as_str().unwrap().into())).collect()
    }

    #[tokio::test]
    async fn rename_merge_and_forget_a_comp() {
        let (app, dir) = app("overrides");
        boards(&app);
        rebuild(&app);
        let (_, list) = call(&app, Method::GET, "/api/comps", "").await;
        assert_eq!(names(&list).len(), 2);

        // Rename: shown at once, and kept through a rebuild.
        let x = "T_X%7CTFT99_X5";
        let (status, body) = call(&app, Method::POST, &format!("/api/comps/{x}/rename"), r#"{"name":"  My comp "}"#).await;
        assert_eq!(status, 200);
        assert_eq!(body["override"]["name"], "My comp");
        let (_, list) = call(&app, Method::GET, "/api/comps", "").await;
        assert!(names(&list).contains(&("T_X|TFT99_X5".into(), "My comp".into())), "{:?}", names(&list));
        let (_, got) = call(&app, Method::GET, &format!("/api/comps/{x}/override"), "").await;
        assert_eq!(got["override"]["name"], "My comp");

        // Merge Y into X: applied at the next build.
        let y = "T_Y%7CTFT99_Y5";
        let (status, body) = call(&app, Method::POST, &format!("/api/comps/{y}/merge"), r#"{"into":"T_X|TFT99_X5"}"#).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["override"]["mergeInto"], "T_X|TFT99_X5");
        assert!(body["note"].as_str().unwrap().contains("next stats build"));
        rebuild(&app);
        let (_, list) = call(&app, Method::GET, "/api/comps", "").await;
        assert_eq!(names(&list), [("T_X|TFT99_X5".to_string(), "My comp".to_string())]);
        assert_eq!(list["rows"][0]["games"], 240);

        // Refused: into itself, a circle, a body without the field.
        let (status, body) = call(&app, Method::POST, &format!("/api/comps/{x}/merge"), r#"{"into":"T_Y|TFT99_Y5"}"#).await;
        assert_eq!((status, body["error"].as_str().unwrap().contains("circle")), (400, true));
        let (status, _) = call(&app, Method::POST, &format!("/api/comps/{x}/merge"), r#"{"into":"T_X|TFT99_X5"}"#).await;
        assert_eq!(status, 400);
        let (status, _) = call(&app, Method::POST, &format!("/api/comps/{x}/rename"), "not json").await;
        assert_eq!(status, 400);

        // Forget the merge: Y comes back at the next build.
        let (status, _) = call(&app, Method::DELETE, &format!("/api/comps/{y}/override"), "").await;
        assert_eq!(status, 200);
        rebuild(&app);
        let (_, list) = call(&app, Method::GET, "/api/comps", "").await;
        assert_eq!(names(&list).len(), 2);
        // Wrong methods do not reach these routes.
        let (status, _) = call(&app, Method::GET, &format!("/api/comps/{x}/rename"), "").await;
        assert_eq!(status, 404);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn the_clusters_method_names_comps_and_lists_variants() {
        let (app, dir) = app("method");
        boards(&app);
        let (_, m) = call(&app, Method::GET, "/api/settings/comps", "").await;
        assert_eq!(m["method"], "baseline");
        let (status, _) = call(&app, Method::POST, "/api/settings/comps", r#"{"method":"kmeans"}"#).await;
        assert_eq!(status, 400);
        let (status, m) = call(&app, Method::POST, "/api/settings/comps", r#"{"method":"clusters"}"#).await;
        assert_eq!((status, m["method"].as_str(), m["changed"].as_bool()), (200, Some("clusters"), Some(true)));
        rebuild(&app);

        let (_, list) = call(&app, Method::GET, "/api/comps", "").await;
        let rows = names(&list);
        assert_eq!(rows.len(), 2, "{rows:?}");
        for (id, name) in &rows {
            assert!(id.starts_with('c') && !id.contains('|'), "{id}");
            // Named after the top trait and the three-item units: "T X X4 X5" style, from prettified ids.
            assert!(name.starts_with("T X ") || name.starts_with("T Y "), "{name}");
        }
        let x = &rows.iter().find(|r| r.1.starts_with("T X")).unwrap().0;
        let (status, v) = call(&app, Method::GET, &format!("/api/comps/{x}/variants"), "").await;
        assert_eq!(status, 200);
        let vs = v["variants"].as_array().unwrap();
        assert_eq!(vs.len(), 2, "{v}");
        assert_eq!(vs[0]["games"], 80);
        assert!(vs[0]["carries"][0]["name"].is_string());
        assert_eq!(vs[0]["pickRate"], 80.0 / 30.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
