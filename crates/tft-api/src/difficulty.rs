//! The difficulty you give a comp (PRD 2 C10): a tag you set yourself, stored in `comp_difficulty` (migration 6)
//! by comp id, never rebuilt. The playstyle tags (reroll, fast 8, fast 9) come from the data; this one does not.
//!
//! - `POST /api/comps/:id/difficulty` `{ level }`: `easy`, `medium` or `hard`; null or "" clears it. Answers with
//!   your marks (as `POST /api/lists/mark` does), which carry every comp's difficulty as `difficulty`.
//! - On `/api/comps`, each row gets `difficulty` (null when not set), and `?difficulty=` filters by it, with
//!   `none` for comps you have not tagged.

use crate::routes::Obj;
use crate::{App, json, json_status, server_error};
use anyhow::Result;
use axum::body::Body;
use axum::http::Method;
use axum::response::Response;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The levels, easiest first, as stored and as the API sends them.
pub const LEVELS: [&str; 3] = ["easy", "medium", "hard"];
/// The `?difficulty=` value for comps with no difficulty set.
pub const UNSET: &str = "none";

/// `POST /api/comps/:id/difficulty`: the comp id, decoded.
pub(crate) fn route(method: &Method, path: &str) -> Option<String> {
    let seg: Vec<&str> = path.split('/').collect();
    match seg.as_slice() {
        ["", "api", "comps", c, "difficulty"] if method == Method::POST && !c.is_empty() => Some(crate::js::try_decode_component(c)),
        _ => None,
    }
}

/// Every comp's difficulty, by comp id.
pub fn all(conn: &Connection) -> Result<Obj> {
    let mut st = conn.prepare_cached("SELECT comp_key, level FROM comp_difficulty ORDER BY comp_key")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Map::new();
    for r in rows {
        let (k, v) = r?;
        out.insert(k, json!(v));
    }
    Ok(out)
}

/// Sets (or with `None`, clears) one comp's difficulty.
pub fn set(conn: &Connection, id: &str, level: Option<&str>) -> Result<()> {
    match level {
        Some(l) => conn.execute("INSERT OR REPLACE INTO comp_difficulty (comp_key, level) VALUES (?, ?)", [id, l])?,
        None => conn.execute("DELETE FROM comp_difficulty WHERE comp_key = ?", [id])?,
    };
    Ok(())
}

/// Reads `{ level }`: `Ok(None)` clears, `Err` is the message for a 400.
fn parse(body: &[u8]) -> std::result::Result<Option<String>, String> {
    let ask = "Send { \"level\": \"easy\" }, \"medium\" or \"hard\", or { \"level\": null } to clear it.".to_string();
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(b)) => match b.get("level") {
            None => Err(ask),
            Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.is_empty() => Ok(None),
            Some(Value::String(s)) if LEVELS.contains(&s.as_str()) => Ok(Some(s.clone())),
            Some(_) => Err(ask),
        },
        _ => Err(ask),
    }
}

/// Answers `POST /api/comps/:id/difficulty`.
pub(crate) async fn handle(app: &Arc<App>, id: String, body: Body) -> Response {
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return server_error(e.into()),
    };
    let level = match parse(&bytes) {
        Ok(l) => l,
        Err(message) => return json_status(400, json!({ "error": message }).as_object().unwrap().clone()),
    };
    let app2 = Arc::clone(app);
    let done = tokio::task::spawn_blocking(move || -> Result<Obj> {
        let db = app2.write.lock().unwrap();
        set(&db, &id, level.as_deref())?;
        crate::routes::marks(&db)
    })
    .await;
    match done {
        Ok(Ok(body)) => json(body),
        Ok(Err(e)) => server_error(e),
        Err(e) => server_error(e.into()),
    }
}

/// Whether a comp row passes `?difficulty=` (`want`), from its `difficulty` field.
pub fn keep(want: Option<&str>, row: &Obj) -> bool {
    let have = row.get("difficulty").and_then(Value::as_str);
    match want {
        None => true,
        Some(UNSET) => have.is_none(),
        Some(w) => have == Some(w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_level_and_refuses_anything_else() {
        assert_eq!(parse(br#"{"level":"hard"}"#), Ok(Some("hard".into())));
        assert_eq!(parse(br#"{"level":null}"#), Ok(None));
        assert_eq!(parse(br#"{"level":""}"#), Ok(None));
        for bad in [&br#"{"level":"HARD"}"#[..], br#"{"level":3}"#, br#"{}"#, b"null", b"not json"] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert_eq!(route(&Method::POST, "/api/comps/A%7CB/difficulty"), Some("A|B".into()));
        assert_eq!(route(&Method::GET, "/api/comps/A/difficulty"), None);
        assert_eq!(route(&Method::POST, "/api/comps//difficulty"), None);
    }

    #[test]
    fn set_clear_and_filter() {
        let dir = std::env::temp_dir().join(format!("tft-api-difficulty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = tft_store::open(&dir.join("t.db")).unwrap();
        set(&conn, "A|B", Some("hard")).unwrap();
        set(&conn, "C|D", Some("easy")).unwrap();
        set(&conn, "C|D", Some("medium")).unwrap(); // replaces
        assert_eq!(Value::Object(all(&conn).unwrap()), json!({ "A|B": "hard", "C|D": "medium" }));
        assert_eq!(crate::routes::marks(&conn).unwrap()["difficulty"], json!({ "A|B": "hard", "C|D": "medium" }));
        set(&conn, "A|B", None).unwrap();
        set(&conn, "never set", None).unwrap(); // clearing nothing is fine
        assert_eq!(Value::Object(all(&conn).unwrap()), json!({ "C|D": "medium" }));

        let row = |d: Value| json!({ "id": "x", "difficulty": d }).as_object().unwrap().clone();
        assert!(keep(None, &row(json!("hard"))));
        assert!(keep(Some("hard"), &row(json!("hard"))));
        assert!(!keep(Some("easy"), &row(json!("hard"))));
        assert!(keep(Some(UNSET), &row(Value::Null)));
        assert!(!keep(Some(UNSET), &row(json!("easy"))));
        assert!(!keep(Some("easy"), &row(Value::Null)));
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
