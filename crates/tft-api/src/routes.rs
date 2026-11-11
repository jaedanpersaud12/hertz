//! The routes of `src/server/index.ts`, one function per route, in the same order and with the same queries.
//! Each returns the JSON body; the server in `lib.rs` turns it into a response. The summoner routes are in `me.rs`.

use crate::describe::{Kind, describe};
use crate::js::{self, num, number, number_of, patch_order};
use anyhow::Result;
use crate::jsval::{self, JsValue};
use rusqlite::{Connection, OptionalExtension, ToSql, types::Value as SqlValue, types::ValueRef};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tft_core::config::{BUCKETS, scope_key};
use tft_core::order::locale_cmp;

pub type Obj = Map<String, Value>;

mod comp_page;
mod flex;

/// Time windows inside a patch, as `WINDOWS` in `src/config.ts`. '' is the whole patch.
const WINDOWS: [(&str, &str, Option<i64>); 2] = [("", "Whole patch", None), ("3d", "Last 3 days", Some(3))];

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn to_json(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => num(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!({ "type": "Buffer", "data": b }),
    }
}

/// Every row of a query as an object keyed by column name, as better-sqlite3's `.all()` gives them.
fn rows(conn: &Connection, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Obj>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let mut q = stmt.query(params)?;
    while let Some(r) = q.next()? {
        let mut o = Map::new();
        for (i, n) in names.iter().enumerate() {
            // Like better-sqlite3, a later column of the same name wins.
            o.insert(n.clone(), to_json(r.get_ref(i)?));
        }
        out.push(o);
    }
    Ok(out)
}
fn row(conn: &Connection, sql: &str, params: &[&dyn ToSql]) -> Result<Option<Obj>> {
    Ok(rows(conn, sql, params)?.into_iter().next())
}

/// A field as a JavaScript number (`undefined` and null read as NaN and 0 where arithmetic would).
fn f(o: &Obj, k: &str) -> f64 {
    match o.get(k) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Null) => 0.0,
        Some(v) => number_of(v),
        None => f64::NAN,
    }
}
fn s<'a>(o: &'a Obj, k: &str) -> &'a str {
    o.get(k).and_then(Value::as_str).unwrap_or("")
}
fn g(o: &Obj, k: &str) -> Value {
    o.get(k).cloned().unwrap_or(Value::Null)
}
fn merge(mut a: Obj, b: Obj) -> Obj {
    a.extend(b);
    a
}
fn with(mut a: Obj, pairs: &[(&str, Value)]) -> Obj {
    for (k, v) in pairs {
        a.insert((*k).into(), v.clone());
    }
    a
}

/// `rate()`: games, average place, win and top-4 rates, and share of all boards.
fn rate(r: &Obj, total: f64) -> Obj {
    let games = f(r, "games");
    let mut o = Map::new();
    o.insert("games".into(), g(r, "games"));
    o.insert("avgPlace".into(), num(f(r, "sum_place") / games));
    o.insert("winRate".into(), num(f(r, "wins") / games));
    o.insert("top4Rate".into(), num(f(r, "top4") / games));
    o.insert("frequency".into(), if total != 0.0 && !total.is_nan() { num(games / total) } else { json!(0) });
    o
}

pub struct Scope {
    pub patch: String,
    pub bucket: &'static str,
    pub window: &'static str,
    pub key: String,
    pub rank_min: i64,
    pub since: i64,
    pub min: f64,
    pub comps: i64,
    pub matches: i64,
    pub built_at: Option<i64>,
    pub patches: Vec<String>,
    pub prev_patch: Option<String>,
}

/// Resolves ?patch=&bucket=&window=&min= against what has been built.
pub fn scope(conn: &Connection, q: Option<&str>) -> Result<Scope> {
    let mut patches: Vec<String> =
        rows(conn, "SELECT DISTINCT patch FROM stat_meta", &[])?.iter().map(|r| s(r, "patch").to_string()).collect();
    patches.sort_by(|a, b| patch_order(a, b));
    let patch = js::query(q, "patch").filter(|p| !p.is_empty()).or_else(|| patches.first().cloned()).unwrap_or_default();
    let bq = js::query(q, "bucket");
    let bucket = BUCKETS.iter().find(|b| Some(b.key) == bq.as_deref()).unwrap_or(&BUCKETS[0]);
    let wq = js::query(q, "window");
    let window = WINDOWS.iter().find(|w| Some(w.0) == wq.as_deref()).unwrap_or(&WINDOWS[0]);
    let key = scope_key(bucket.key, window.0);
    let meta = row(conn, "SELECT matches, comps, built_at FROM stat_meta WHERE patch = ? AND bucket = ?", &[&patch, &key])?;
    let min = js::query(q, "min").map(|m| number(&m)).unwrap_or(10.0);
    let min = if min.is_nan() { f64::NAN } else { min.max(1.0) };
    let idx = patches.iter().position(|p| *p == patch).map(|i| i as i64).unwrap_or(-1);
    Ok(Scope {
        prev_patch: patches.get((idx + 1) as usize).cloned(),
        since: window.2.map(|d| now_ms() - d * 86_400_000).unwrap_or(0),
        comps: meta.as_ref().map(|m| f(m, "comps") as i64).unwrap_or(0),
        matches: meta.as_ref().map(|m| f(m, "matches") as i64).unwrap_or(0),
        built_at: meta.as_ref().and_then(|m| m.get("built_at")).and_then(Value::as_i64),
        patch,
        bucket: bucket.key,
        window: window.0,
        key,
        rank_min: bucket.min,
        min,
        patches,
    })
}

fn public(s: &Scope) -> Obj {
    let v = json!({
        "patch": s.patch, "bucket": s.bucket, "window": s.window, "min": num(s.min), "comps": s.comps,
        "matches": s.matches, "builtAt": s.built_at, "patches": s.patches, "prevPatch": s.prev_patch,
    });
    match v {
        Value::Object(o) => o,
        _ => unreachable!(),
    }
}

/// The minimum-games filter as a bound value: NaN binds as NULL, as better-sqlite3 binds it.
fn min_param(s: &Scope) -> Option<f64> {
    (!s.min.is_nan()).then_some(s.min)
}

// ---- Comps ---------------------------------------------------------------------

/// Built comps per scope, kept until the stats are rebuilt (like `compCache`).
pub type CompCache = Mutex<HashMap<String, std::sync::Arc<Vec<Obj>>>>;

fn group(rs: Vec<Obj>) -> HashMap<String, Vec<Obj>> {
    let mut m: HashMap<String, Vec<Obj>> = HashMap::new();
    for r in rs {
        m.entry(s(&r, "comp_key").to_string()).or_default().push(r);
    }
    m
}

/// `Math.round`.
fn js_round(x: f64) -> f64 {
    // JavaScript rounds halves up (towards +Infinity); everything else to nearest.
    let r = x.round();
    if (r - x).abs() == 0.5 { x.floor() + 1.0 } else { r }
}

fn load_comps(conn: &Connection, sc: &Scope, cache: &CompCache) -> Result<std::sync::Arc<Vec<Obj>>> {
    // Tiers and trends are worked out after a scope is written (they read other scopes), so they are in the key too.
    let finished = row(
        conn,
        "SELECT COUNT(*) n, TOTAL(score) s, TOTAL(prev_place) p, TOTAL(d3_place) d FROM stat_comps WHERE patch = ? AND bucket = ?",
        &[&sc.patch, &sc.key],
    )?
    .map(|r| format!("{}|{}|{}|{}", g(&r, "n"), g(&r, "s"), g(&r, "p"), g(&r, "d")))
    .unwrap_or_default();
    let cache_key =
        format!("{}|{}|{}|{finished}", sc.patch, sc.key, sc.built_at.map(|b| b.to_string()).unwrap_or("null".into()));
    if let Some(hit) = cache.lock().unwrap().get(&cache_key) {
        return Ok(hit.clone());
    }
    let k: [&dyn ToSql; 2] = [&sc.patch, &sc.key];
    let unit_rows = group(rows(conn, "SELECT * FROM stat_comp_units WHERE patch = ? AND bucket = ? ORDER BY games DESC", &k)?);
    let item_rows = group(rows(conn, "SELECT * FROM stat_comp_items WHERE patch = ? AND bucket = ? ORDER BY games DESC", &k)?);
    let trait_rows = group(rows(conn, "SELECT * FROM stat_comp_traits WHERE patch = ? AND bucket = ? ORDER BY games DESC", &k)?);
    let empty: Vec<Obj> = Vec::new();

    let mut comps = Vec::new();
    for c in rows(conn, "SELECT * FROM stat_comps WHERE patch = ? AND bucket = ?", &k)? {
        let comp_key = s(&c, "comp_key").to_string();
        let us = unit_rows.get(&comp_key).unwrap_or(&empty);
        let its = item_rows.get(&comp_key).unwrap_or(&empty);
        let cg = f(&c, "games");
        let (trait_id, carry_id) =
            crate::comp_admin::trait_and_carry(&comp_key, trait_rows.get(&comp_key).unwrap_or(&empty), us, cg);

        // Core units: on the board in at least half the games. Small or messy comps fall back to the seven most common.
        let mut core: Vec<&Obj> = us.iter().filter(|u| f(u, "games") / cg >= 0.5).take(10).collect();
        if core.len() < 4 {
            core = us.iter().take(7).collect();
        }
        let mut units = Vec::new();
        for u in &core {
            let ug = f(u, "games");
            let avg_items = f(u, "sum_items") / ug;
            let cid = s(u, "character_id");
            let mut items = Vec::new();
            if avg_items >= 1.5 {
                for i in its.iter().filter(|i| s(i, "character_id") == cid).take(3) {
                    items.push(Value::Object(with(describe(conn, Kind::Item, s(i, "item"), false)?, &[("freq", num(f(i, "games") / ug))])));
                }
            }
            units.push(with(
                describe(conn, Kind::Unit, cid, false)?,
                &[
                    ("star", num(js_round(f(u, "sum_star") / ug).max(1.0))),
                    ("freq", num(ug / cg)),
                    ("avgItems", num(avg_items)),
                    ("items", Value::Array(items)),
                ],
            ));
        }
        let cost = |o: &Obj| match o.get("cost") {
            Some(Value::Null) | None => 9.0,
            Some(v) => number_of(v),
        };
        units.sort_by(|a, b| {
            let d = cost(a) - cost(b);
            if d != 0.0 && !d.is_nan() {
                d.partial_cmp(&0.0).unwrap()
            } else {
                locale_cmp(s(a, "name"), s(b, "name"))
            }
        });

        let core_ids: HashSet<&str> = core.iter().map(|u| s(u, "character_id")).collect();
        let mut flex = Vec::new();
        for u in us.iter().filter(|u| !core_ids.contains(s(u, "character_id")) && f(u, "games") / cg >= 0.15).take(8) {
            flex.push(Value::Object(with(describe(conn, Kind::Unit, s(u, "character_id"), false)?, &[("freq", num(f(u, "games") / cg))])));
        }

        // Traits active in at least half the games, shown at their most common level.
        let mut order: Vec<String> = Vec::new();
        let mut by_trait: HashMap<String, Vec<&Obj>> = HashMap::new();
        for t in trait_rows.get(&comp_key).unwrap_or(&empty) {
            let name = s(t, "name").to_string();
            if !by_trait.contains_key(&name) {
                order.push(name.clone());
            }
            by_trait.entry(name).or_default().push(t);
        }
        let mut firsts: Vec<&Obj> = order
            .iter()
            .map(|n| &by_trait[n])
            .filter(|levels| levels.iter().map(|l| f(l, "games")).sum::<f64>() / cg >= 0.5)
            .map(|levels| levels[0])
            .collect();
        firsts.sort_by(|a, b| (f(b, "num_units") - f(a, "num_units")).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut traits = Vec::new();
        for t in firsts.into_iter().take(9) {
            traits.push(Value::Object(with(
                describe(conn, Kind::Trait, s(t, "name"), false)?,
                &[("units", g(t, "num_units")), ("style", g(t, "style"))],
            )));
        }

        // Carries: the units holding three items in most games. They name the comp.
        let mut carry_rows: Vec<&Obj> = us.iter().filter(|u| f(u, "full_items") / cg >= 0.4).collect();
        carry_rows.sort_by(|a, b| (f(b, "full_items") - f(a, "full_items")).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        let carry_ids: Vec<String> = if carry_rows.is_empty() {
            vec![carry_id]
        } else {
            carry_rows.iter().take(2).map(|u| s(u, "character_id").to_string()).collect()
        };
        let mut carries = Vec::new();
        for id in &carry_ids {
            carries.push(describe(conn, Kind::Unit, id, false)?);
        }
        let tr = if trait_id == "Flex" { None } else { Some(describe(conn, Kind::Trait, &trait_id, false)?) };

        let avg_level = f(&c, "sum_level") / cg;
        let main = us.iter().find(|u| s(u, "character_id") == s(&carries[0], "id"));
        let carry_cost = match carries[0].get("cost") {
            Some(Value::Null) | None => 0.0,
            Some(v) => number_of(v),
        };
        // Playstyle (PRD 2 C10), from the data: `reroll` when the main carry costs 1 to 3 and averages 2.5 stars
        // or more; otherwise by the average final level, `fast9` from 8.6, `fast8` from 7.6, else `level`.
        let reroll = (1.0..=3.0).contains(&carry_cost) && main.is_some_and(|m| f(m, "sum_star") / f(m, "games") >= 2.5);
        let lvl = js_round(avg_level);
        let play = if reroll {
            json!({ "kind": "reroll", "label": format!("Reroll lvl {lvl}") })
        } else if avg_level >= 8.6 {
            json!({ "kind": "fast9", "label": "Fast 9" })
        } else if avg_level >= 7.6 {
            json!({ "kind": "fast8", "label": "Fast 8" })
        } else {
            json!({ "kind": "level", "label": format!("Lvl {lvl}") })
        };

        let mut name = vec![tr.as_ref().map(|t| s(t, "name").to_string()).unwrap_or_else(|| "Flex".into())];
        name.extend(carries.iter().map(|u| s(u, "name").to_string()));
        let comp = with(
            Map::new(),
            &[
                ("id", json!(comp_key)),
                ("name", json!(crate::comp_admin::name_override(conn, &comp_key)?.unwrap_or_else(|| name.join(" ")))),
                ("trait", tr.map(Value::Object).unwrap_or(Value::Null)),
                ("carries", Value::Array(carries.into_iter().map(Value::Object).collect())),
                ("units", Value::Array(units.into_iter().map(Value::Object).collect())),
                ("flex", Value::Array(flex)),
                ("traits", Value::Array(traits)),
                ("avgLevel", num(avg_level)),
                ("play", play),
                // Boards per lobby, like MetaTFT's pick rate.
                ("pickRate", if sc.matches != 0 { num(cg / sc.matches as f64) } else { json!(0) }),
            ],
        );
        let mut comp = merge(comp, rate(&c, sc.comps as f64));
        comp.extend(crate::tierlist::fields(&c, f(&c, "sum_place") / cg).into_iter().map(|(k, v)| (k.to_string(), v)));
        comps.push(comp);
    }
    let comps = std::sync::Arc::new(comps);
    let mut c = cache.lock().unwrap();
    if c.len() > 24 {
        c.clear();
    }
    c.insert(cache_key, comps.clone());
    Ok(comps)
}

pub(crate) fn marks(conn: &Connection) -> Result<Obj> {
    let rs = rows(conn, "SELECT comp_key, fav, hidden FROM comp_marks", &[])?;
    let truthy = |r: &Obj, k: &str| f(r, k) != 0.0 && !f(r, k).is_nan();
    let pinned = match row(conn, "SELECT value FROM kv WHERE key = ?", &[&"pinned"])? {
        Some(r) => serde_json::from_str(s(&r, "value"))?,
        None => Value::Null,
    };
    Ok(with(
        Map::new(),
        &[
            ("favs", rs.iter().filter(|r| truthy(r, "fav")).map(|r| g(r, "comp_key")).collect()),
            ("hidden", rs.iter().filter(|r| truthy(r, "hidden")).map(|r| g(r, "comp_key")).collect()),
            ("pinned", pinned),
            ("difficulty", Value::Object(crate::difficulty::all(conn)?)), // PRD 2 C10
        ],
    ))
}

fn arr(xs: Vec<Obj>) -> Value {
    Value::Array(xs.into_iter().map(Value::Object).collect())
}

pub fn comps(conn: &Connection, q: Option<&str>, cache: &CompCache) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let all = load_comps(conn, &sc, cache)?;
    let listed: Vec<&Obj> = all.iter().filter(|r| f(r, "games") >= sc.min).collect();
    // The tier list's filters (PRD 2 C11). With none asked for, every listed comp.
    let filters = crate::tierlist::Filters::from_query(q);
    // Your difficulty tags (C10) on each row, and ?difficulty= among the filters.
    let tags = crate::difficulty::all(conn)?;
    let want = js::query(q, "difficulty").filter(|v| !v.is_empty());
    let rows: Vec<Obj> = listed
        .iter()
        .filter(|r| filters.keep(r))
        .map(|r| with((*r).clone(), &[("difficulty", tags.get(s(r, "id")).cloned().unwrap_or(Value::Null))]))
        .filter(|r| crate::difficulty::keep(want.as_deref(), r))
        .collect();
    Ok(with(merge(public(&sc), marks(conn)?), &[("rows", arr(rows)), ("options", crate::tierlist::options(&listed))]))
}

pub fn comp(conn: &Connection, q: Option<&str>, id: &str, cache: &CompCache) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let all = load_comps(conn, &sc, cache)?;
    let Some(comp) = all.iter().find(|r| s(r, "id") == id) else {
        return Ok(with(public(&sc), &[("comp", Value::Null)]));
    };
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    let comp_games = f(comp, "games");
    let comp_avg = f(comp, "avgPlace");
    let levels: Vec<Value> = rows(conn, "SELECT * FROM stat_comp_levels WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY level", &k)?
        .iter()
        .filter(|l| f(l, "level") > 0.0)
        .map(|l| {
            json!({ "level": g(l, "level"), "games": g(l, "games"), "share": num(f(l, "games") / comp_games),
                "avgPlace": num(f(l, "sum_place") / f(l, "games")) })
        })
        .collect();
    let items = rows(conn, "SELECT * FROM stat_comp_items WHERE patch = ? AND bucket = ? AND comp_key = ? ORDER BY games DESC", &k)?;
    let unit_games: HashMap<String, f64> =
        rows(conn, "SELECT character_id, games FROM stat_comp_units WHERE patch = ? AND bucket = ? AND comp_key = ?", &k)?
            .iter()
            .map(|u| (s(u, "character_id").to_string(), f(u, "games")))
            .collect();
    let mut with_items: Vec<&Obj> = comp
        .get("units")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|u| f(u, "avgItems") >= 1.5)
        .collect();
    with_items.sort_by(|a, b| (f(b, "avgItems") - f(a, "avgItems")).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut builds = Vec::new();
    for u in with_items {
        let uid = s(u, "id");
        let mut out = Vec::new();
        for i in items.iter().filter(|i| s(i, "character_id") == uid).take(8) {
            let ig = f(i, "games");
            let avg = f(i, "sum_place") / ig;
            out.push(Value::Object(with(
                describe(conn, Kind::Item, s(i, "item"), false)?,
                &[
                    ("games", g(i, "games")),
                    ("freq", num(ig / unit_games.get(uid).copied().unwrap_or(ig))),
                    ("avgPlace", num(avg)),
                    ("placeChange", num(avg - comp_avg)),
                ],
            )));
        }
        builds.push(json!({ "unit": u, "items": out }));
    }
    let body = with(
        public(&sc),
        &[("comp", Value::Object(comp.clone())), ("levels", Value::Array(levels)), ("builds", Value::Array(builds))],
    );
    let body = with(body, &comp_page::sections(conn, &sc, id, comp)?);
    Ok(with(body, &flex::sections(conn, &sc, id, comp)?))
}

// ---- Units ---------------------------------------------------------------------

pub fn units(conn: &Connection, q: Option<&str>) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let mut out = Vec::new();
    for r in rows(conn, "SELECT * FROM stat_units WHERE patch = ? AND bucket = ? AND games >= ?", &[&sc.patch, &sc.key, &min_param(&sc)])? {
        let d = merge(describe(conn, Kind::Unit, s(&r, "character_id"), false)?, rate(&r, sc.comps as f64));
        out.push(with(d, &[("avgStar", g(&r, "avg_star"))]));
    }
    Ok(with(public(&sc), &[("rows", arr(out))]))
}

pub fn unit(conn: &Connection, q: Option<&str>, id: &str, cache: &CompCache) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let k: [&dyn ToSql; 3] = [&sc.patch, &sc.key, &id];
    let Some(unit) = row(conn, "SELECT * FROM stat_units WHERE patch = ? AND bucket = ? AND character_id = ?", &k)? else {
        return Ok(with(
            public(&sc),
            &[
                ("unit", Value::Object(describe(conn, Kind::Unit, id, true)?)),
                ("items", json!([])),
                ("builds", json!([])),
                ("stars", json!([])),
                ("itemCounts", json!([])),
                ("comps", json!([])),
                ("trend", json!([])),
                ("prev", Value::Null),
            ],
        ));
    };
    let ug = f(&unit, "games");
    let dist = rows(conn, "SELECT * FROM stat_unit_dist WHERE patch = ? AND bucket = ? AND character_id = ?", &k)?;
    let fold = |field: &str| -> Value {
        let mut keys: Vec<f64> = Vec::new();
        let mut m: Vec<(f64, f64, Value)> = Vec::new(); // games, sum_place, the key as stored
        for d in &dist {
            let key = f(d, field);
            match keys.iter().position(|x| *x == key) {
                Some(i) => {
                    m[i].0 += f(d, "games");
                    m[i].1 += f(d, "sum_place");
                }
                None => {
                    keys.push(key);
                    m.push((f(d, "games"), f(d, "sum_place"), g(d, field)));
                }
            }
        }
        let mut idx: Vec<usize> = (0..m.len()).collect();
        idx.sort_by(|a, b| (keys[*a] - keys[*b]).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        Value::Array(
            idx.into_iter()
                .map(|i| json!({ "value": m[i].2, "games": num(m[i].0), "share": num(m[i].0 / ug), "avgPlace": num(m[i].1 / m[i].0) }))
                .collect(),
        )
    };

    let mp = min_param(&sc);
    let items = rows(conn, "SELECT * FROM stat_unit_items WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?", &[&sc.patch, &sc.key, &id, &mp])?;
    let builds = rows(
        conn,
        "SELECT * FROM stat_unit_builds WHERE patch = ? AND bucket = ? AND character_id = ? AND games >= ?\n    ORDER BY games DESC LIMIT 80",
        &[&sc.patch, &sc.key, &id, &mp],
    )?;

    // Same unit, same rank filter, every patch we have stats for.
    let mut trend_rows = rows(
        conn,
        "SELECT su.patch, su.games, su.sum_place, su.wins, su.top4, sm.comps FROM stat_units su\n    JOIN stat_meta sm ON sm.patch = su.patch AND sm.bucket = su.bucket WHERE su.bucket = ? AND su.character_id = ?",
        &[&sc.key, &id],
    )?;
    trend_rows.sort_by(|a, b| patch_order(s(b, "patch"), s(a, "patch")));
    let trend: Vec<Obj> =
        trend_rows.iter().map(|r| with(rate(r, f(r, "comps")), &[("patch", g(r, "patch"))])).collect();
    let prev = trend
        .iter()
        .find(|t| Some(s(t, "patch")) == sc.prev_patch.as_deref())
        .map(|p| json!({ "patch": g(p, "patch"), "avgPlace": g(p, "avgPlace"), "frequency": g(p, "frequency") }))
        .unwrap_or(Value::Null);

    let all = load_comps(conn, &sc, cache)?;
    let mut cs: Vec<&Obj> = all
        .iter()
        .filter(|c| c.get("units").and_then(Value::as_array).is_some_and(|us| us.iter().any(|u| u.get("id").and_then(Value::as_str) == Some(id))))
        .collect();
    cs.sort_by(|a, b| (f(b, "games") - f(a, "games")).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let comps: Vec<Value> = cs
        .into_iter()
        .take(12)
        .map(|c| {
            json!({ "id": g(c, "id"), "name": g(c, "name"), "games": g(c, "games"), "avgPlace": g(c, "avgPlace"),
                "pickRate": g(c, "pickRate"), "units": g(c, "units"), "play": g(c, "play") })
        })
        .collect();

    let mut item_out = Vec::new();
    for r in &items {
        let d = merge(describe(conn, Kind::Item, s(r, "item"), false)?, rate(r, ug));
        item_out.push(with(d, &[("placeChange", g(r, "place_change"))]));
    }
    let mut build_out = Vec::new();
    for r in &builds {
        let mut its = Vec::new();
        for i in s(r, "build").split('|') {
            its.push(Value::Object(describe(conn, Kind::Item, i, false)?));
        }
        let d = merge(with(Map::new(), &[("id", g(r, "build")), ("items", Value::Array(its))]), rate(r, ug));
        build_out.push(with(d, &[("placeChange", g(r, "place_change"))]));
    }
    let full = merge(describe(conn, Kind::Unit, id, true)?, rate(&unit, sc.comps as f64));
    Ok(with(
        public(&sc),
        &[
            ("unit", Value::Object(with(full, &[("avgStar", g(&unit, "avg_star"))]))),
            ("prev", prev),
            ("stars", fold("star")),
            ("itemCounts", fold("n_items")),
            ("trend", arr(trend)),
            ("comps", Value::Array(comps)),
            ("items", arr(item_out)),
            ("builds", arr(build_out)),
        ],
    ))
}

// ---- Traits --------------------------------------------------------------------

pub fn traits(conn: &Connection, q: Option<&str>) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let rs = rows(conn, "SELECT * FROM stat_traits WHERE patch = ? AND bucket = ?", &[&sc.patch, &sc.key])?;
    let total = sc.comps as f64;
    let mut out = Vec::new();
    for r in rs.iter().filter(|r| f(r, "tier_current") == 0.0 && f(r, "games") >= sc.min) {
        let mut levels: Vec<&Obj> =
            rs.iter().filter(|l| s(l, "name") == s(r, "name") && f(l, "tier_current") > 0.0).collect();
        levels.sort_by(|a, b| (f(a, "tier_current") - f(b, "tier_current")).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        let levels: Vec<Obj> = levels
            .into_iter()
            .map(|l| merge(with(Map::new(), &[("level", g(l, "tier_current")), ("units", g(l, "num_units"))]), rate(l, total)))
            .collect();
        let d = merge(describe(conn, Kind::Trait, s(r, "name"), false)?, rate(r, total));
        out.push(with(d, &[("levels", arr(levels))]));
    }
    Ok(with(public(&sc), &[("rows", arr(out))]))
}

// ---- Items ---------------------------------------------------------------------

pub fn items(conn: &Connection, q: Option<&str>) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let rs = rows(conn, "SELECT * FROM stat_items WHERE patch = ? AND bucket = ? AND games >= ?", &[&sc.patch, &sc.key, &min_param(&sc)])?;
    // Top five holders per item, from one pass over the unit-item table.
    let mut holders: HashMap<String, Vec<String>> = HashMap::new();
    for h in rows(conn, "SELECT item, character_id FROM stat_unit_items WHERE patch = ? AND bucket = ? ORDER BY games DESC", &[&sc.patch, &sc.key])? {
        let list = holders.entry(s(&h, "item").to_string()).or_default();
        if list.len() < 5 {
            list.push(s(&h, "character_id").to_string());
        }
    }
    let mut out = Vec::new();
    for r in &rs {
        let mut hs = Vec::new();
        for id in holders.get(s(r, "item")).into_iter().flatten() {
            hs.push(Value::Object(describe(conn, Kind::Unit, id, false)?));
        }
        let d = merge(describe(conn, Kind::Item, s(r, "item"), false)?, rate(r, sc.comps as f64));
        out.push(with(d, &[("placeChange", g(r, "place_change")), ("holders", Value::Array(hs))]));
    }
    Ok(with(public(&sc), &[("rows", arr(out))]))
}

pub fn item(conn: &Connection, q: Option<&str>, id: &str) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let full = describe(conn, Kind::Item, id, true)?;
    let mut recipe = Vec::new();
    if let Some(Value::Array(parts)) = full.get("composition") {
        for p in parts {
            let pid = match p {
                Value::String(x) => x.clone(),
                other => other.to_string(),
            };
            recipe.push(Value::Object(describe(conn, Kind::Item, &pid, false)?));
        }
    }
    let Some(item) = row(conn, "SELECT * FROM stat_items WHERE patch = ? AND bucket = ? AND item = ?", &[&sc.patch, &sc.key, &id])? else {
        return Ok(with(
            public(&sc),
            &[("item", Value::Object(full)), ("recipe", Value::Array(recipe)), ("holders", json!([])), ("prev", Value::Null)],
        ));
    };
    let prev = match &sc.prev_patch {
        Some(pp) => row(
            conn,
            "SELECT si.*, sm.comps FROM stat_items si JOIN stat_meta sm ON sm.patch = si.patch AND sm.bucket = si.bucket\n        WHERE si.patch = ? AND si.bucket = ? AND si.item = ?",
            &[pp, &sc.key, &id],
        )?,
        None => None,
    };
    let ig = f(&item, "games");
    let mut hs = Vec::new();
    for r in rows(conn, "SELECT * FROM stat_unit_items WHERE patch = ? AND bucket = ? AND item = ? AND games >= ?", &[&sc.patch, &sc.key, &id, &min_param(&sc)])? {
        let d = merge(describe(conn, Kind::Unit, s(&r, "character_id"), false)?, rate(&r, ig));
        hs.push(with(d, &[("placeChange", g(&r, "place_change"))]));
    }
    let prev = prev
        .map(|p| json!({ "patch": sc.prev_patch, "avgPlace": num(f(&p, "sum_place") / f(&p, "games")), "frequency": num(f(&p, "games") / f(&p, "comps")) }))
        .unwrap_or(Value::Null);
    let full = merge(full, rate(&item, sc.comps as f64));
    Ok(with(
        public(&sc),
        &[
            ("item", Value::Object(with(full, &[("placeChange", g(&item, "place_change"))]))),
            ("recipe", Value::Array(recipe)),
            ("prev", prev),
            ("holders", arr(hs)),
        ],
    ))
}

// ---- Explorer: live questions against the raw boards -----------------------------

pub fn explore(conn: &Connection, q: Option<&str>) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let ids = |key: &str, max: usize| -> Vec<String> {
        js::query(q, key)
            .unwrap_or_default()
            .split(',')
            .map(|x| x.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}').to_string())
            .filter(|x| !x.is_empty())
            .take(max)
            .collect()
    };
    let units = ids("units", 4);
    let traits = ids("traits", 3);
    if units.is_empty() {
        return Ok(with(public(&sc), &[("summary", Value::Null), ("partners", json!([])), ("items", json!([])), ("ms", json!(0))]));
    }

    let t0 = std::time::Instant::now();
    let ms = |t0: std::time::Instant| json!(t0.elapsed().as_millis() as i64);
    conn.execute_batch("DROP TABLE IF EXISTS temp.hit")?;
    let sql = format!(
        "\n    CREATE TEMP TABLE hit AS\n    SELECT DISTINCT pa.id, pa.placement FROM units u0\n    JOIN participants pa ON pa.id = u0.participant_id\n    JOIN matches m ON m.match_id = pa.match_id\n    WHERE u0.character_id = ? AND m.patch = ? AND m.rank >= ? AND m.game_datetime >= ? AND {}\n    {}\n    {}\n  ",
        ranked_only("m"),
        units[1..].iter().map(|_| "AND EXISTS (SELECT 1 FROM units x WHERE x.participant_id = pa.id AND x.character_id = ?)").collect::<Vec<_>>().join("\n"),
        traits.iter().map(|_| "AND EXISTS (SELECT 1 FROM traits t WHERE t.participant_id = pa.id AND t.name = ?)").collect::<Vec<_>>().join("\n"),
    );
    let mut params: Vec<&dyn ToSql> = vec![&units[0], &sc.patch, &sc.rank_min, &sc.since];
    params.extend(units[1..].iter().map(|u| u as &dyn ToSql));
    params.extend(traits.iter().map(|t| t as &dyn ToSql));
    conn.prepare(&sql)?.execute(params.as_slice())?;

    let sum = row(conn, "SELECT COUNT(*) games, COALESCE(SUM(placement), 0) sum_place, COALESCE(SUM(placement = 1), 0) wins, COALESCE(SUM(placement <= 4), 0) top4 FROM hit", &[])?
        .unwrap_or_default();
    let games = f(&sum, "games");
    if games == 0.0 || games.is_nan() {
        return Ok(with(public(&sc), &[("summary", json!({ "games": 0 })), ("partners", json!([])), ("items", json!([])), ("ms", ms(t0))]));
    }
    let partners = rows(
        conn,
        "\n    SELECT character_id, COUNT(*) games, SUM(placement) sum_place, SUM(placement = 1) wins, SUM(placement <= 4) top4\n    FROM (SELECT DISTINCT u.participant_id, u.character_id, h.placement FROM hit h JOIN units u ON u.participant_id = h.id)\n    GROUP BY character_id ORDER BY games DESC LIMIT 40",
        &[],
    )?;
    let items = rows(
        conn,
        "\n    SELECT item, COUNT(*) games, SUM(placement) sum_place, SUM(placement = 1) wins, SUM(placement <= 4) top4\n    FROM (SELECT DISTINCT ui.participant_id, ui.item, h.placement FROM hit h JOIN unit_items ui ON ui.participant_id = h.id WHERE ui.character_id = ?)\n    GROUP BY item ORDER BY games DESC LIMIT 30",
        &[&units[0]],
    )?;
    conn.execute_batch("DROP TABLE IF EXISTS temp.hit")?;

    let summary = rate(&sum, sc.comps as f64);
    let avg = f(&summary, "avgPlace");
    let mut p_out = Vec::new();
    for p in partners.iter().filter(|p| !units.iter().any(|u| u == s(p, "character_id"))) {
        let d = merge(describe(conn, Kind::Unit, s(p, "character_id"), false)?, rate(p, games));
        p_out.push(with(d, &[("placeChange", num(f(p, "sum_place") / f(p, "games") - avg))]));
    }
    let mut i_out = Vec::new();
    for i in &items {
        let d = merge(describe(conn, Kind::Item, s(i, "item"), false)?, rate(i, games));
        i_out.push(with(d, &[("placeChange", num(f(i, "sum_place") / f(i, "games") - avg))]));
    }
    Ok(with(
        public(&sc),
        &[
            ("summary", Value::Object(summary)),
            ("ms", ms(t0)),
            ("partners", arr(p_out)),
            ("items", arr(i_out)),
            ("itemsOn", Value::Object(describe(conn, Kind::Unit, &units[0], false)?)),
        ],
    ))
}

// ---- Static data for pickers and the team builder ---------------------------------

pub fn static_data(conn: &Connection) -> Result<Obj> {
    let all = |kind: Kind| -> Result<Vec<Obj>> {
        let ids = rows(conn, "SELECT id FROM static_data WHERE kind = ? ORDER BY name", &[&kind.as_str()])?;
        ids.iter().map(|r| describe(conn, kind, s(r, "id"), false)).collect()
    };
    // No download yet: fall back to whatever has shown up in matches, so pickers still work.
    let seen = |sql: &str, kind: Kind| -> Result<Vec<Obj>> {
        let mut out = rows(conn, sql, &[])?
            .iter()
            .map(|r| describe(conn, kind, &match r.get("id") {
                Some(Value::String(x)) => x.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            }, false))
            .collect::<Result<Vec<_>>>()?;
        out.sort_by(|a, b| locale_cmp(s(a, "name"), s(b, "name")));
        Ok(out)
    };
    let units: Vec<Obj> = all(Kind::Unit)?
        .into_iter()
        .filter(|u| {
            let cost = number_of(&g(u, "cost"));
            (1.0..=5.0).contains(&cost) && u.get("traits").and_then(js::length_of).is_some_and(|n| n > 0)
        })
        .collect();
    let traits = all(Kind::Trait)?;
    let loaded = !units.is_empty();
    Ok(with(
        Map::new(),
        &[
            ("loaded", json!(loaded)),
            ("units", arr(if units.is_empty() { seen("SELECT DISTINCT character_id id FROM stat_units", Kind::Unit)? } else { units })),
            ("traits", arr(if traits.is_empty() { seen("SELECT DISTINCT name id FROM stat_traits", Kind::Trait)? } else { traits })),
            ("items", arr(all(Kind::Item)?)),
        ],
    ))
}

// ---- Your lists ----------------------------------------------------------------

fn teams(conn: &Connection) -> Result<Value> {
    let mut teams = Vec::new();
    for t in rows(conn, "SELECT id, name, board, created FROM saved_teams ORDER BY created DESC", &[])? {
        let board: Value = serde_json::from_str(s(&t, "board"))?;
        teams.push(json!({ "id": g(&t, "id"), "name": g(&t, "name"), "created": g(&t, "created"), "board": board }));
    }
    Ok(Value::Array(teams))
}

pub fn lists(conn: &Connection) -> Result<Obj> {
    Ok(with(marks(conn)?, &[("teams", teams(conn)?)]))
}

/// A value bound the way better-sqlite3 binds it: numbers as doubles, `undefined` as NULL, a one-entry array as
/// its entry. Booleans, objects and other arrays make it throw, which the TypeScript API answers with a 500.
fn bind(v: Option<&JsValue>) -> Result<SqlValue> {
    Ok(match v {
        None | Some(JsValue::Null) => SqlValue::Null,
        Some(JsValue::Str(x)) => SqlValue::Text(x.clone()),
        Some(JsValue::Num(x)) if x.is_nan() => SqlValue::Null,
        Some(JsValue::Num(x)) => SqlValue::Real(*x),
        // An array is the list of parameters: one entry binds as that entry.
        Some(JsValue::Arr(xs)) if xs.len() == 1 && !matches!(xs[0], JsValue::Arr(_)) => bind(Some(&xs[0]))?,
        Some(other) => anyhow::bail!("SQLite3 can only bind numbers, strings, bigints, buffers, and null (got {})", other.stringify()),
    })
}

/// `const { a, b } = body`: a body of `null` throws.
fn fields(body: &JsValue) -> Result<&JsValue> {
    if *body == JsValue::Null {
        anyhow::bail!("Cannot destructure a body of null");
    }
    Ok(body)
}

/// POST /api/lists/mark: { key, fav?, hidden? }.
pub fn mark(conn: &Connection, body: &JsValue) -> Result<Obj> {
    let b = fields(body)?;
    let key = bind(b.get("key"))?;
    conn.execute("INSERT OR IGNORE INTO comp_marks (comp_key) VALUES (?)", [&key])?;
    if let Some(fav) = b.get("fav") {
        conn.execute("UPDATE comp_marks SET fav = ? WHERE comp_key = ?", rusqlite::params![i64::from(fav.truthy()), key])?;
    }
    if let Some(hidden) = b.get("hidden") {
        conn.execute("UPDATE comp_marks SET hidden = ? WHERE comp_key = ?", rusqlite::params![i64::from(hidden.truthy()), key])?;
    }
    marks(conn)
}

/// POST /api/lists/pin: { key } pins a comp; a falsy key unpins.
pub fn pin(conn: &Connection, body: &JsValue) -> Result<Obj> {
    match fields(body)?.get("key").filter(|k| k.truthy()) {
        Some(key) => crate::me::kv_set_js(conn, "pinned", key)?,
        None => {
            conn.execute("DELETE FROM kv WHERE key = 'pinned'", [])?;
        }
    }
    marks(conn)
}

/// POST /api/teams: { name, board }. The name is cut to 60 UTF-16 units, as `String(name).slice(0, 60)` cuts it.
pub fn save_team(conn: &Connection, body: &JsValue) -> Result<Obj> {
    let b = fields(body)?;
    let units: Vec<u16> = jsval::js_string(b.get("name")).encode_utf16().take(60).collect();
    // A cut through an emoji leaves half of it; better-sqlite3 stores that as its three-byte (CESU-8) form.
    let mut name = Vec::new();
    for c in char::decode_utf16(units.iter().copied()) {
        match c {
            Ok(c) => name.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Err(e) => {
                let u = u32::from(e.unpaired_surrogate());
                name.extend_from_slice(&[0xE0 | (u >> 12) as u8, 0x80 | ((u >> 6) & 0x3F) as u8, 0x80 | (u & 0x3F) as u8]);
            }
        }
    }
    let name = if name.is_empty() { b"Untitled team".to_vec() } else { name };
    let board = b.get("board").map(JsValue::stringify); // JSON.stringify(undefined) binds as NULL and fails NOT NULL
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as f64;
    conn.execute(
        "INSERT INTO saved_teams (name, board, created) VALUES (?,?,?)",
        rusqlite::params![rusqlite::types::ToSqlOutput::Borrowed(ValueRef::Text(&name)), board, created],
    )?;
    Ok(with(Map::new(), &[("teams", teams(conn)?)]))
}

/// DELETE /api/teams/:id. The id goes through `Number()`, so "1.0" and "0x1" delete team 1.
pub fn delete_team(conn: &Connection, id: &str) -> Result<Obj> {
    let id = js::number(id);
    conn.execute("DELETE FROM saved_teams WHERE id = ?", [(!id.is_nan()).then_some(id)])?;
    Ok(with(Map::new(), &[("teams", teams(conn)?)]))
}

// ---- Status --------------------------------------------------------------------

/// Ranked matches only (E12), the same SQL text as `rankedOnly()` in `src/db.ts` and `ranked_only` in tft-stats.
/// Other game modes are stored but not counted.
fn ranked_only(table: &str) -> String {
    let id = tft_core::config::Config::from_env().ranked_queue_id;
    format!("({table}.queue_id IS NULL OR {table}.queue_id = {id})")
}

pub fn meta(conn: &Connection, q: Option<&str>) -> Result<Obj> {
    let sc = scope(conn, q)?;
    let totals = row(
        conn,
        &format!(
            "SELECT (SELECT COUNT(*) FROM matches m WHERE {r}) matches,\n    (SELECT COUNT(*) FROM participants pa JOIN matches m ON m.match_id = pa.match_id WHERE {r}) comps,\n    (SELECT COUNT(*) FROM players) players, (SELECT COUNT(*) FROM match_queue WHERE status = 0) queued,\n    (SELECT MAX(game_datetime) FROM matches) newest, (SELECT COUNT(*) FROM static_data) staticRows",
            r = ranked_only("m")
        ),
        &[],
    )?;
    let by_patch = rows(
        conn,
        &format!(
            "SELECT patch, COUNT(*) matches FROM matches m WHERE {}\n    GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT 8",
            ranked_only("m")
        ),
        &[],
    )?;
    Ok(with(
        public(&sc),
        &[
            ("totals", totals.map(Value::Object).unwrap_or(Value::Null)),
            ("byPatch", arr(by_patch)),
            ("buckets", json!(BUCKETS.iter().map(|b| b.key).collect::<Vec<_>>())),
            ("windows", json!(WINDOWS.iter().map(|w| json!({ "key": w.0, "label": w.1 })).collect::<Vec<_>>())),
        ],
    ))
}

/// The icon URL stored for an id, if any (`!s?.icon` is a 404).
pub fn icon_source(conn: &Connection, kind: &str, id: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached("SELECT icon FROM static_data WHERE kind = ? AND id = ? COLLATE NOCASE")?
        .query_row((kind, id), |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .filter(|i| !i.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_math_round() {
        for (x, want) in [(2.5, 3.0), (2.4999, 2.0), (7.5, 8.0), (1.0, 1.0), (0.49999999999999994, 0.0)] {
            assert_eq!(js_round(x), want, "{x}");
        }
    }
}
