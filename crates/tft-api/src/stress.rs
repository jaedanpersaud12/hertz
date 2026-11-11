//! Dev-only stress data for the interface, as `src/server/worstcase.ts` made it. Never active under `tft serve`
//! or `tft run` without `--dev`.
//!
//! With `npm run dev` (or `npm run demo`), adding `?data=worst | empty | one | many` to a GET `/api/*` call
//! reshapes the real answer into a worst realistic case, so the real components render it. The switch at the
//! bottom of the app (dev only) sets that parameter for you.

use crate::engine::server_label;
use crate::js::num;
use crate::jsval::number_string;
use serde_json::{Map, Value, json};
use tft_core::config::{BUCKETS, PLATFORMS, TIERS};

type Obj = Map<String, Value>;

pub const MODES: [&str; 4] = ["worst", "empty", "one", "many"];

// Real names, chosen for what they do to a layout: long, two letters, apostrophes, an ampersand, other scripts.
const UNITS: [&str; 14] = [
    "Aurelion Sol", "Nunu & Willump", "Vi", "Kog'Maw", "Renata Glasc", "아우렐리온 솔", "Twisted Fate", "Jax",
    "Тахм Кенч", "Miss Fortune", "オレリオン・ソル", "Bel'Veth", "ออเรเลียน โซล", "Dr. Mundo",
];
const ITEMS: [&str; 11] = [
    "Rabadon's Deathcap", "Guinsoo's Rageblade", "Tactician's Crown", "Unstable Treasure Chest",
    "Battle Academia Emblem", "B.F. Sword", "Zz'Rot Portal", "Rabadons Todeshaube", "라바돈의 죽음모자",
    "Needlessly Large Rod", "Protector's Vow",
];
const TRAITS: [&str; 11] = [
    "Battle Academia", "Star Guardian", "Soul Fighter", "Mighty Mech", "The Crew", "Supreme Cells", "Wraith",
    "Stance Master", "Crystal Gambit", "Monster Trainer", "별 수호자",
];
// Riot IDs allow 16 characters plus a 5-character tag. W is the widest letter.
const PLAYERS: [Option<&str>; 8] = [
    Some("WWWWWWWWWWWWWWWW#WWWWW"), Some("숨겨진 드래곤 마스터#KR1"), Some("Jo#1"), Some("محمد العبدالله#EUNE"), None,
    Some("Aleksandra Wiśnie#EUW"), Some("ngọc ánh nguyễn#VN2"), Some("x#NA1"),
];
const DAY: f64 = 86_400_000.0;

/// `[...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)`: per code point, its first UTF-16 unit.
fn hash(s: &str) -> usize {
    let mut h: u64 = 7;
    for c in s.chars() {
        let mut buf = [0u16; 2];
        let unit = c.encode_utf16(&mut buf)[0];
        h = (h * 31 + u64::from(unit)) & 0xffff_ffff;
    }
    h as usize
}

/// `String(v)` for a JSON value, `undefined` when missing.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(xs)) => xs
            .iter()
            .map(|x| if x.is_null() { String::new() } else { js_string(Some(x)) })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// JavaScript truthiness.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// Visit every object in a JSON tree: the object first, then whatever its values are after the visit.
fn walk(v: &mut Value, f: &mut dyn FnMut(&mut Obj)) {
    match v {
        Value::Array(xs) => xs.iter_mut().for_each(|x| walk(x, f)),
        Value::Object(o) => {
            f(o);
            o.values_mut().for_each(|x| walk(x, f));
        }
        _ => {}
    }
}

fn rename(o: &mut Obj) {
    let (Some(Value::String(id)), Some(Value::String(_))) = (o.get("id"), o.get("name")) else { return };
    let h = hash(id);
    if o.contains_key("cost") {
        o.insert("name".into(), json!(UNITS[h % UNITS.len()]));
        if h % 5 == 0 {
            // not in the downloaded set data yet
            o.insert("cost".into(), Value::Null);
            o.insert("icon".into(), Value::Null);
        }
        if o.get("traits").is_some_and(Value::is_array) && h % 3 == 0 {
            o.insert("traits".into(), json!(["Battle Academia", "Star Guardian", "Monster Trainer", "Crystal Gambit"]));
        }
    } else if o.contains_key("type") {
        o.insert("name".into(), json!(ITEMS[h % ITEMS.len()]));
    } else if o.contains_key("breakpoints") || o.contains_key("style") {
        o.insert("name".into(), json!(TRAITS[h % TRAITS.len()]));
    }
}

/// Rows that share a table get different extremes, the way real data mixes them.
fn extremes() -> [Vec<(&'static str, Value)>; 3] {
    [
        vec![
            ("games", json!(12_345_678)), ("avgPlace", json!(1)), ("winRate", json!(1)), ("top4Rate", json!(1)),
            ("frequency", json!(1)), ("pickRate", json!(8)), ("placeChange", json!(-3.21)),
            // The tier list (PRD 2 C3, C4): top tier, rising fast on both trends.
            ("tier", json!("S")), ("score", json!(12.345_678)), ("prevPlace", json!(4.21)), ("d3Place", json!(1)),
            ("patchDelta", json!(-3.21)), ("d3Delta", json!(-3.456)), ("rising", json!(true)),
            ("difficulty", json!("hard")), ("baseGames", json!(12_345_678)), // C10, C8
        ],
        vec![
            ("games", json!(1)), ("avgPlace", json!(8)), ("winRate", json!(0)), ("top4Rate", json!(0)),
            ("frequency", json!(0.00004)), ("pickRate", json!(0.0003)), ("placeChange", json!(3.4)),
            ("tier", json!("D")), ("score", json!(-12.345_678)), ("prevPlace", json!(4.6)), ("d3Place", json!(8)),
            ("patchDelta", json!(3.4)), ("d3Delta", json!(3.456)), ("rising", json!(false)),
            ("difficulty", json!("medium")), ("baseGames", json!(10)),
        ],
        // Not graded yet, and new this patch: nothing to compare with.
        vec![
            ("placeChange", Value::Null), ("avgStar", json!(3)), ("tier", Value::Null), ("score", Value::Null),
            ("prevPlace", Value::Null), ("d3Place", Value::Null), ("patchDelta", Value::Null), ("d3Delta", Value::Null),
            ("rising", json!(false)), ("difficulty", Value::Null),
        ],
    ]
}

fn extreme(rows: Option<&mut Value>) {
    let Some(Value::Array(rows)) = rows else { return };
    for (r, e) in rows.iter_mut().zip(extremes()) {
        let Value::Object(r) = r else { continue };
        for (k, v) in e {
            if r.contains_key(k) {
                r.insert(k.into(), v);
            }
        }
    }
}

/// `{ ...base, ...extra }`.
fn spread(base: &Value, extra: Vec<(&str, Value)>) -> Value {
    let mut o = base.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        o.insert(k.into(), v);
    }
    Value::Object(o)
}

fn comp_and_players(o: &mut Obj) {
    // A comp: its name is built from its trait and carries, and one comp gets the fullest possible board.
    let is_comp = truthy(o.get("play"))
        && o.get("units").is_some_and(Value::is_array)
        && o.get("carries").is_some_and(Value::is_array);
    if is_comp {
        let trait_name = match o.get("trait").and_then(|t| t.get("name")) {
            None | Some(Value::Null) => "Flex".to_string(),
            Some(v) => js_string(Some(v)),
        };
        let mut name = vec![trait_name];
        for u in o["carries"].as_array().unwrap() {
            name.push(match u.get("name") {
                None | Some(Value::Null) => String::new(),
                v => js_string(v),
            });
        }
        o.insert("name".into(), json!(name.join(" ")));
        let units = o["units"].as_array().unwrap().clone();
        if hash(&js_string(o.get("id"))) % 2 == 0 && !units.is_empty() {
            let full = |u: Value| {
                let items: Vec<Value> =
                    (0..3).map(|n| json!({ "id": format!("i{n}"), "name": ITEMS[n], "icon": null })).collect();
                spread(&u, vec![("star", json!(3)), ("items", Value::Array(items))])
            };
            let new_units: Vec<Value> = (0..10)
                .map(|n| {
                    let u = &units[n % units.len()];
                    full(spread(u, vec![("id", json!(format!("{}~{n}", js_string(u.get("id")))))]))
                })
                .collect();
            if let Some(Value::Array(traits)) = o.get("traits")
                && !traits.is_empty()
            {
                let traits = traits.clone();
                let out: Vec<Value> = (0..9)
                    .map(|n| {
                        spread(&traits[n % traits.len()], vec![
                            ("id", json!(format!("t{n}"))),
                            ("name", json!(TRAITS[n])),
                            ("units", json!(if n == 0 { 10 } else { 2 + (n % 4) })),
                            ("style", json!(1 + (n % 5))),
                        ])
                    })
                    .collect();
                o.insert("traits".into(), Value::Array(out));
            }
            if o.get("flex").is_some_and(Value::is_array) {
                let flex: Vec<Value> = (0..8)
                    .map(|n| {
                        spread(&new_units[n], vec![
                            ("id", json!(format!("f{n}"))),
                            ("freq", num(0.15 + n as f64 / 30.0)),
                            ("items", json!([])),
                        ])
                    })
                    .collect();
                o.insert("flex".into(), Value::Array(flex));
            }
            o.insert("units".into(), Value::Array(new_units));
            o.insert("play".into(), json!({ "kind": "reroll", "label": "Reroll lvl 10" }));
        }
    }
    // A player in a lobby.
    if o.contains_key("placement") && o.contains_key("damage") {
        let name = match o.get("name") {
            None | Some(Value::Null) => String::new(),
            v => js_string(v),
        };
        let key = format!("{}{}{name}", js_string(o.get("damage")), js_string(o.get("placement")));
        o.insert("name".into(), PLAYERS[hash(&key) % PLAYERS.len()].map_or(Value::Null, |p| json!(p)));
        o.insert("level".into(), json!(10));
        o.insert("damage".into(), json!(248));
    }
    if o.contains_key("queue") && o.contains_key("length") && hash(&js_string(o.get("id"))) % 3 == 0 {
        o.insert("queue".into(), json!("Queue 6120"));
        o.insert("length".into(), json!(0));
    }
}

fn worst(path: &str, body: &mut Obj) {
    let mut whole = Value::Object(std::mem::take(body));
    walk(&mut whole, &mut rename);
    let Value::Object(b) = &mut whole else { unreachable!() };
    for key in ["rows", "items", "builds", "holders", "partners", "comps", "trend"] {
        extreme(b.get_mut(key));
    }
    walk(&mut whole, &mut comp_and_players);
    let Value::Object(b) = whole else { unreachable!() };
    *body = b;
    if path.starts_with("/api/comps/") {
        comp_page(body, true);
    }
    depth_page(path, body, "worst");

    let long_ago = num(now_ms() - 400.0 * DAY);
    if path == "/api/meta" {
        if let Some(Value::Object(t)) = body.get_mut("totals") {
            t.insert("matches".into(), json!(12_345_678));
            t.insert("comps".into(), json!(98_765_432));
            t.insert("players".into(), json!(1_284_003));
            t.insert("queued".into(), json!(1));
            t.insert("newest".into(), long_ago.clone());
        }
        body.insert("comps".into(), json!(98_765_432));
        body.insert("matches".into(), json!(12_345_678));
        body.insert(
            "byPatch".into(),
            json!([{ "patch": "16.22", "matches": 12_345_678 }, { "patch": "16.21", "matches": 1 }, { "patch": "unknown", "matches": 0 }]),
        );
    }
    if path == "/api/me" && truthy(body.get("name")) {
        for (k, v) in [
            ("name", json!("WWWWWWWWWWWWWWWW")), ("tag", json!("WWWWW")), ("tier", json!("GRANDMASTER")),
            ("lp", json!(2417)), ("recent", json!([8, 8, 8, 8, 8, 8, 8, 8, 8, 8])), ("avgPlace", json!(8)),
            ("error", json!("Riot did not answer on americas after several tries. Check your connection, or try again in a few minutes.")),
        ] {
            body.insert(k.into(), v);
        }
    }
    if path == "/api/lists"
        && let Some(Value::Array(teams)) = body.get_mut("teams")
    {
        for (i, t) in teams.iter_mut().enumerate() {
            if let Value::Object(t) = t {
                let name = if i > 0 { "x" } else { "Battle Academia reroll into Aurelion Sol if Nunu hits early!!" };
                t.insert("name".into(), json!(name));
                t.insert("created".into(), long_ago.clone());
            }
        }
    }
}

const LISTS: [&str; 25] = [
    "rows", "matches", "teams", "favs", "hidden", "builds", "levels", "partners", "items", "holders", "comps", "trend",
    "stars", "itemCounts", "units", "traits", "recent", "byPatch", "levelling", "carries", "special",
    "addAt9", "swaps", "variants", "teammates",
];

fn shrink(path: &str, body: &mut Obj, keep: usize) {
    depth_page(path, body, if keep == 0 { "empty" } else { "one" });
    for key in LISTS {
        if let Some(Value::Array(xs)) = body.get_mut(key) {
            xs.truncate(keep);
        }
    }
    let one = |k: bool| if k { json!(1) } else { json!(0) };
    if path == "/api/meta" {
        if let Some(Value::Object(t)) = body.get_mut("totals") {
            for v in t.values_mut() {
                *v = one(keep > 0);
            }
            t.insert("newest".into(), if keep > 0 { num(now_ms() - DAY) } else { Value::Null });
        }
        body.insert("comps".into(), json!(keep));
        body.insert("matches".into(), json!(keep));
        if keep == 0 {
            body.insert("patches".into(), json!([]));
            body.insert("builtAt".into(), Value::Null);
        }
    }
    if path == "/api/static" {
        body.insert("loaded".into(), json!(keep > 0));
    }
    if path == "/api/me" && truthy(body.get("name")) {
        if keep > 0 {
            let first = body.get("recent").and_then(|r| r.get(0)).cloned().unwrap_or(Value::Null);
            body.insert("avgPlace".into(), first);
        } else {
            body.insert("tier".into(), Value::Null);
            body.insert("avgPlace".into(), Value::Null);
        }
    }
    if path.starts_with("/api/explore") && truthy(body.get("summary")) {
        let summary = if keep > 0 {
            spread(&body["summary"], vec![("games", json!(1)), ("frequency", json!(0.0000003))])
        } else {
            json!({ "games": 0 })
        };
        body.insert("summary".into(), summary);
    }
}

/// The comp page sections of `/api/comps/:id` (PRD 2 C5, C6, C7, C9) at the most the API lists: two carries,
/// three builds and five items each, six holders, twelve special items, all three placement bands with every level.
/// `worst` also gives the rows the extremes and leaves one band with no known level or round.
fn comp_page(body: &mut Obj, worst: bool) {
    let fill = |xs: &mut Vec<Value>, n: usize| {
        if xs.is_empty() {
            return;
        }
        let base = xs.clone();
        *xs = (0..n)
            .map(|i| {
                let mut v = base[i % base.len()].clone();
                if i >= base.len() {
                    walk(&mut v, &mut |o: &mut Obj| {
                        if let Some(Value::String(id)) = o.get("id") {
                            let id = format!("{id}~{i}");
                            o.insert("id".into(), json!(id));
                        }
                    });
                }
                v
            })
            .collect();
    };
    if let Some(Value::Array(carries)) = body.get_mut("carries") {
        fill(carries, 2);
        for c in carries.iter_mut() {
            for (key, n) in [("builds", 3), ("items", 5)] {
                if let Some(Value::Array(xs)) = c.get_mut(key) {
                    fill(xs, n);
                }
                if worst {
                    extreme(c.get_mut(key));
                }
            }
        }
    }
    if let Some(Value::Array(holders)) = body.get_mut("holders") {
        fill(holders, 2);
        for h in holders.iter_mut() {
            if let Some(Value::Array(xs)) = h.get_mut("units") {
                fill(xs, 6);
            }
            if worst {
                extreme(h.get_mut("units"));
            }
        }
    }
    if let Some(Value::Array(xs)) = body.get_mut("special") {
        fill(xs, 12);
    }
    if worst {
        extreme(body.get_mut("special"));
    }
    // C8: six units added at 9, eight swaps; C2: four variants (/api/comps/:id/variants).
    for (key, n) in [("addAt9", 6), ("swaps", 8), ("variants", 4)] {
        if let Some(Value::Array(xs)) = body.get_mut(key) {
            fill(xs, n);
        }
        if worst {
            extreme(body.get_mut(key));
        }
    }
    if let Some(Value::Array(bands)) = body.get_mut("levelling")
        && !bands.is_empty()
    {
        let labels = [(1, "1st–2nd"), (2, "3rd–4th"), (3, "5th–8th")];
        *bands = labels
            .iter()
            .map(|(band, label)| {
                let unknown = worst && *band == 2;
                let levels: Vec<Value> = if unknown {
                    Vec::new()
                } else {
                    (1..=10).map(|l| json!({ "level": l, "games": 1_000 * l, "share": l as f64 / 55.0 })).collect()
                };
                json!({
                    "band": band, "label": label, "games": if worst { 12_345_678 } else { 400 },
                    "avgPlace": if worst { 8.0 } else { 1.5 + *band as f64 * 1.5 },
                    "commonLevel": if unknown { Value::Null } else { json!(10) },
                    "avgRound": if unknown { Value::Null } else { json!(41.7) },
                    "levels": levels,
                })
            })
            .collect();
    }
}

/// The item page's stats depth (PRD 3 U5, U6, U7): five best holders per item, six builds per component, every
/// emblem level. `worst` gives them the extremes and the fallback weights; `keep` (one, empty) shrinks the nested
/// lists the way [`shrink`] does the top-level ones.
fn items_page(path: &str, body: &mut Obj, worst: bool, keep: Option<usize>) {
    let fill = |xs: &mut Vec<Value>, n: usize| {
        if xs.is_empty() {
            return;
        }
        let base = xs.clone();
        *xs = (0..n)
            .map(|i| {
                let mut v = base[i % base.len()].clone();
                if i >= base.len()
                    && let Value::Object(o) = &mut v
                {
                    let id = format!("{}~{i}", js_string(o.get("id")));
                    o.insert("id".into(), json!(id));
                }
                v
            })
            .collect();
    };
    let ends = |xs: &mut Vec<Value>, key: &str, best: Value, worst_v: Value| {
        for (x, v) in xs.iter_mut().zip([best, worst_v]) {
            if let Value::Object(o) = x {
                o.insert(key.into(), v);
            }
        }
        if let Some(Value::Object(o)) = xs.first_mut() {
            o.insert("games".into(), json!(12_345_678));
        }
    };
    let nested = |body: &mut Obj, list: &str, key: &str, n: usize, extreme: &dyn Fn(&mut Vec<Value>)| {
        if let Some(Value::Array(rows)) = body.get_mut(list) {
            for r in rows.iter_mut() {
                if let Some(Value::Array(xs)) = r.get_mut(key) {
                    match keep {
                        Some(k) => xs.truncate(k),
                        None => {
                            fill(xs, n);
                            if worst {
                                extreme(xs);
                            }
                        }
                    }
                }
            }
        }
    };
    if path == "/api/items" {
        nested(body, "rows", "bestHolders", 5, &|xs| ends(xs, "placeChange", json!(-3.21), json!(3.4)));
    }
    if path == "/api/items/components" {
        nested(body, "rows", "items", 6, &|xs| ends(xs, "weight", json!(-3.21), json!(3.4)));
        if worst {
            body.insert("basis".into(), json!("all"));
        }
    }
    if path.starts_with("/api/items/")
        && let Some(Value::Object(e)) = body.get_mut("emblem")
    {
        let bp = e.get("breakpoint").cloned().filter(|b| !b.is_null());
        if let Some(Value::Array(levels)) = e.get_mut("levels") {
            match keep {
                // One row: the breakpoint's own, so the answer and the table agree.
                Some(1) if bp.is_some() => *levels = vec![bp.unwrap()],
                Some(k) => levels.truncate(k),
                None if worst && !levels.is_empty() => {
                    let base = levels[0].clone();
                    *levels = (0..=5)
                        .map(|l| {
                            spread(&base, vec![
                                ("level", json!(l)), ("units", if l == 0 { Value::Null } else { json!(l * 2) }),
                                ("games", json!(if l == 1 { 12_345_678 } else { 1 })), ("wins", json!(if l == 1 { 12_345_678 } else { 0 })),
                                ("avgPlace", json!(if l == 1 { 1 } else { 8 })), ("winRate", json!(if l == 1 { 1 } else { 0 })),
                                ("top4Rate", json!(if l == 1 { 1 } else { 0 })),
                            ])
                        })
                        .collect();
                }
                None => {}
            }
        }
        if keep == Some(0) {
            e.insert("breakpoint".into(), Value::Null);
        } else if worst {
            let bp = e.get("levels").and_then(|l| l.get(1)).cloned().unwrap_or(Value::Null);
            e.insert("breakpoint".into(), bp);
        }
    }
}

fn many(body: &mut Obj) {
    if let Some(Value::Array(base)) = body.get("rows")
        && !base.is_empty()
    {
        let rows: Vec<Value> = (0..1000)
            .map(|n| {
                let b = &base[n % base.len()];
                spread(b, vec![("id", json!(format!("{}~{n}", js_string(b.get("id"))))), ("games", json!(1000 - n))])
            })
            .collect();
        body.insert("rows".into(), Value::Array(rows));
    }
}

/// The unit page's grid and teammates and the trait page's breakpoints (PRD 3 U1, U4, U8). The grid keeps its
/// twelve cells in every mode: empty has none played, one has a single played cell, worst mixes the extremes with
/// unplayed cells. Many fills teammates to the ten shown, a trait to eight breakpoints, each with ten units.
fn depth_page(path: &str, body: &mut Obj, mode: &str) {
    let seg: Vec<&str> = path.split('/').collect();
    let unit_page = seg.len() == 4 && seg[2] == "units";
    let trait_page = seg.len() == 4 && seg[2] == "traits";
    if unit_page && let Some(Value::Array(cells)) = body.get_mut("grid") {
        for (i, c) in cells.iter_mut().enumerate() {
            let Value::Object(c) = c else { continue };
            let (games, place, top4) = match mode {
                "empty" => (json!(0), Value::Null, Value::Null),
                "one" if i > 0 => (json!(0), Value::Null, Value::Null),
                "one" => (json!(1), json!(8), json!(0)),
                "worst" if i % 3 == 2 => (json!(0), Value::Null, Value::Null),
                "worst" if i == 0 => (json!(12_345_678), json!(1), json!(1)),
                "worst" => (json!(1 + i), json!(8), json!(0)),
                _ => continue,
            };
            c.insert("games".into(), games);
            c.insert("avgPlace".into(), place);
            c.insert("top4Rate".into(), top4);
        }
    }
    if unit_page && mode == "worst" {
        extreme(body.get_mut("teammates"));
        if let Some(Value::Object(r)) = body.get_mut("recent") {
            r.insert("games".into(), json!(1));
            r.insert("avgPlace".into(), json!(8));
            r.insert("frequency".into(), json!(0.00004));
        }
    }
    let fill = |xs: &mut Vec<Value>, n: usize| {
        if xs.is_empty() {
            return;
        }
        let base = xs.clone();
        *xs = (0..n)
            .map(|i| {
                let mut v = base[i % base.len()].clone();
                if let (true, Value::Object(o)) = (i >= base.len(), &mut v)
                    && let Some(Value::String(id)) = o.get("id")
                {
                    let id = format!("{id}~{i}");
                    o.insert("id".into(), json!(id));
                }
                v
            })
            .collect();
    };
    if unit_page && mode == "many" && let Some(Value::Array(xs)) = body.get_mut("teammates") {
        fill(xs, 10);
    }
    if trait_page && (mode == "many" || mode == "worst") && let Some(Value::Array(levels)) = body.get_mut("levels") {
        fill(levels, 8);
        for (i, l) in levels.iter_mut().enumerate() {
            let Value::Object(o) = l else { continue };
            if let Some(Value::Array(us)) = o.get_mut("set").and_then(|s| s.get_mut("units")) {
                fill(us, 10);
            }
            if mode == "worst" && i == 1 {
                o.insert("set".into(), Value::Null); // no unit set for this breakpoint
            }
            o.insert("level".into(), json!(i + 1));
            o.insert("units".into(), json!(2 * (i + 1)));
        }
    }
    if trait_page && mode == "worst" {
        extreme(body.get_mut("levels"));
    }
}

// ---- The Data page's engine routes (`engine.rs`), which the TypeScript server never had -------------------

const HOUR: i64 = 3_600_000;

fn now_ms_int() -> i64 {
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
    let now = now_ms_int();
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

/// Reshapes one answer for `mode`. Anything that is not a JSON object, or a mode that is not listed, is left alone.
/// `/api/health` and `/api/settings/crawl` get their own datasets; every other route the ones `worstcase.ts` made.
pub fn stress(mode: &str, path: &str, body: &mut Value) {
    let Value::Object(b) = body else { return };
    if !MODES.contains(&mode) {
        return;
    }
    if path == "/api/health" || path == "/api/settings/crawl" {
        let taken = std::mem::take(b);
        *b = if path == "/api/health" { stress_health(Some(mode), taken) } else { stress_settings(Some(mode), taken) };
        return;
    }
    match mode {
        "worst" => {
            worst(path, b);
            items_page(path, b, true, None);
        }
        "many" => {
            many(b);
            depth_page(path, b, "many");
            if path.starts_with("/api/comps/") {
                comp_page(b, false);
            }
            items_page(path, b, false, None);
        }
        "one" => {
            shrink(path, b, 1);
            items_page(path, b, false, Some(1));
        }
        "empty" => {
            shrink(path, b, 0);
            items_page(path, b, false, Some(0));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRD 3 U5, U6, U7: the nested item lists fill up and take the extremes under worst, and shrink under one and empty.
    #[test]
    fn item_page_lists_fill_and_shrink() {
        let unit = json!({ "id": "U", "name": "U", "icon": null, "cost": 1, "traits": [], "games": 9, "placeChange": -1 });
        let item = json!({ "id": "I", "name": "I", "icon": null, "type": "craftable", "weight": -0.5, "games": 9 });
        let level = json!({ "level": 1, "units": 2, "games": 9, "wins": 1, "top4": 5, "avgPlace": 3, "winRate": 0.1, "top4Rate": 0.5 });
        let run = |mode: &str, path: &str, body: Value| -> Value {
            let mut b = body;
            stress(mode, path, &mut b);
            b
        };
        let list = json!({ "rows": [{ "id": "I", "name": "I", "type": "craftable", "holders": [], "bestHolders": [unit] }] });
        let w = run("worst", "/api/items", list.clone());
        assert_eq!(w["rows"][0]["bestHolders"].as_array().unwrap().len(), 5);
        assert_eq!(w["rows"][0]["bestHolders"][0]["placeChange"], json!(-3.21));
        assert_eq!(w["rows"][0]["bestHolders"][1]["placeChange"], json!(3.4));
        assert_eq!(run("empty", "/api/items", list.clone())["rows"], json!([]));
        let comps = json!({ "basis": "top", "rows": [{ "id": "C", "name": "C", "type": "component", "items": [item] }] });
        let w = run("worst", "/api/items/components", comps.clone());
        assert_eq!((w["basis"].clone(), w["rows"][0]["items"].as_array().unwrap().len()), (json!("all"), 6));
        assert_eq!(w["rows"][0]["items"][0]["weight"], json!(-3.21));
        assert_eq!(run("one", "/api/items/components", comps)["rows"][0]["items"].as_array().unwrap().len(), 1);
        let detail = json!({ "item": { "id": "E", "type": "emblem" }, "emblem": { "trait": null, "basis": "wins", "breakpoint": level, "levels": [level] } });
        let w = run("worst", "/api/items/E", detail.clone());
        assert_eq!(w["emblem"]["levels"].as_array().unwrap().len(), 6);
        assert_eq!(w["emblem"]["breakpoint"]["level"], json!(1));
        let e = run("empty", "/api/items/E", detail);
        assert_eq!((e["emblem"]["levels"].clone(), e["emblem"]["breakpoint"].clone()), (json!([]), Value::Null));
    }

    #[test]
    fn hashes_like_javascript() {
        // [...'TFT99_Unit1'].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) in Node.
        assert_eq!(hash(""), 7);
        assert_eq!(hash("a"), 7 * 31 + 97);
        // An astral character counts once, by its high surrogate.
        assert_eq!(hash("😀"), 7 * 31 + 0xD83D);
    }

    /// The tier list's fields get the same treatment as the other numbers on a row, so every stress mode covers
    /// the tier letter, both trends and the rising mark, and the filter choices stay for the empty lists.
    #[test]
    fn covers_the_tier_list_fields() {
        let row = |id: &str| json!({ "id": id, "games": 50, "avgPlace": 4.4, "tier": "B", "score": 0.1, "prevPlace": 4.5,
            "d3Place": 4.3, "patchDelta": -0.1, "d3Delta": -0.1, "rising": false });
        let body = json!({ "rows": [row("a"), row("b"), row("c"), row("d")], "options": { "units": [{ "id": "u", "name": "U", "icon": null, "cost": 1 }] } });
        let mut b = body.clone();
        stress("worst", "/api/comps", &mut b);
        assert_eq!((b["rows"][0]["tier"].clone(), b["rows"][0]["patchDelta"].clone(), b["rows"][0]["rising"].clone()), (json!("S"), json!(-3.21), json!(true)));
        assert_eq!((b["rows"][1]["tier"].clone(), b["rows"][1]["d3Delta"].clone()), (json!("D"), json!(3.456)));
        assert_eq!((b["rows"][2]["tier"].clone(), b["rows"][2]["prevPlace"].clone(), b["rows"][2]["patchDelta"].clone()), (Value::Null, Value::Null, Value::Null));
        assert_eq!(b["rows"][3], row("d"), "a fourth row keeps its real values");
        let tagged = json!({ "rows": [{ "id": "a", "difficulty": null }, { "id": "b", "difficulty": null }, { "id": "c", "difficulty": "easy" }] });
        let mut t = tagged.clone();
        stress("worst", "/api/comps", &mut t);
        assert_eq!(t["rows"].as_array().unwrap().iter().map(|r| r["difficulty"].clone()).collect::<Vec<_>>(), [json!("hard"), json!("medium"), Value::Null]);
        assert_ne!(b["options"]["units"][0]["name"], json!("U"), "filter choices get the awkward names too");
        let mut b = body.clone();
        stress("empty", "/api/comps", &mut b);
        assert_eq!((b["rows"].clone(), b["options"]["units"].as_array().unwrap().len()), (json!([]), 1));
        let mut b = body.clone();
        stress("many", "/api/comps", &mut b);
        assert_eq!((b["rows"].as_array().unwrap().len(), b["rows"][999]["tier"].clone()), (1000, json!("B")));
    }

    #[test]
    fn shrinks_lists_and_meta() {
        let mut b = json!({ "rows": [1, 2, 3], "totals": { "matches": 9, "newest": 5 }, "patches": ["1"], "builtAt": 3 });
        stress("empty", "/api/meta", &mut b);
        assert_eq!(b["rows"], json!([]));
        assert_eq!(b["totals"], json!({ "matches": 0, "newest": null }));
        assert_eq!(b["patches"], json!([]));
        assert_eq!(b["builtAt"], Value::Null);
        let mut b = json!({ "rows": [{ "id": "a", "games": 5 }] });
        stress("many", "/api/units", &mut b);
        assert_eq!(b["rows"].as_array().unwrap().len(), 1000);
        assert_eq!(b["rows"][999], json!({ "id": "a~999", "games": 1 }));
        let mut b = json!([1]);
        stress("worst", "/api/units", &mut b);
        assert_eq!(b, json!([1]));
    }

    #[test]
    fn comp_page_sections_fill_to_the_most_shown_and_empty_out() {
        let unit = json!({ "id": "U", "name": "U", "icon": null, "cost": 4, "traits": [] });
        let item = json!({ "id": "I", "name": "I", "icon": null, "type": "emblem" });
        let body = json!({
            "comp": null,
            "levelling": [{ "band": 1, "label": "1st–2nd", "games": 3, "avgPlace": 1.5, "commonLevel": 8, "avgRound": 30, "levels": [] }],
            "carries": [{ "unit": unit, "games": 9, "builds": [{ "items": [item, item, item], "games": 5, "freq": 0.5, "avgPlace": 3, "placeChange": -1 }],
                          "items": [{ "id": "I", "name": "I", "icon": null, "type": "craftable", "games": 10, "freq": 1, "avgPlace": 3, "placeChange": 0.5 }] }],
            "holders": [{ "carry": unit, "games": 9, "top4": 4, "units": [{ "id": "H", "name": "H", "icon": null, "cost": 1, "traits": [], "games": 5, "top4": 2, "top4Share": 0.5, "avgPlace": 4, "sharedTraits": [], "sameRole": true }] }],
            "special": [{ "item": item, "holder": unit, "games": 30, "freq": 0.3, "avgPlace": 4, "placeChange": -0.5 }],
            "addAt9": [{ "unit": unit, "games": 12, "freq": 0.3, "avgPlace": 4, "placeChange": -0.5, "baseGames": 20 }],
            "swaps": [{ "out": unit, "in": unit, "games": 12, "freq": 0.3, "avgPlace": 4, "placeChange": 0.5, "baseGames": 40 }],
        });
        let mut worst = body.clone();
        stress("worst", "/api/comps/X%7CU", &mut worst);
        assert_eq!(worst["carries"].as_array().unwrap().len(), 2);
        assert_eq!(worst["carries"][1]["builds"].as_array().unwrap().len(), 3);
        assert_eq!(worst["carries"][0]["items"].as_array().unwrap().len(), 5);
        assert_eq!(worst["carries"][0]["items"][0]["games"], json!(12_345_678));
        assert_eq!(worst["holders"][0]["units"].as_array().unwrap().len(), 6);
        assert_eq!(worst["special"].as_array().unwrap().len(), 12);
        assert_eq!(worst["special"][1]["placeChange"], json!(3.4));
        assert_eq!(worst["levelling"].as_array().unwrap().len(), 3);
        assert_eq!(worst["levelling"][1]["avgRound"], Value::Null);
        // Copies get their own ids, so lists keyed by id stay unique.
        assert_ne!(worst["special"][0]["item"]["id"], worst["special"][1]["item"]["id"]);
        let mut many = body.clone();
        stress("many", "/api/comps/X%7CU", &mut many);
        assert_eq!(many["levelling"][1]["levels"].as_array().unwrap().len(), 10);
        let mut empty = body.clone();
        stress("empty", "/api/comps/X%7CU", &mut empty);
        assert_eq!(worst["addAt9"].as_array().unwrap().len(), 6);
        assert_eq!(worst["swaps"].as_array().unwrap().len(), 8);
        assert_eq!((worst["swaps"][0]["games"].clone(), worst["swaps"][1]["placeChange"].clone()), (json!(12_345_678), json!(3.4)));
        assert_ne!(worst["swaps"][0]["in"]["id"], worst["swaps"][1]["in"]["id"]);
        for k in ["levelling", "carries", "holders", "special", "addAt9", "swaps"] {
            assert_eq!(empty[k], json!([]), "{k}");
        }
        let mut one = body;
        stress("one", "/api/comps/X%7CU", &mut one);
        assert_eq!(one["special"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn unit_and_trait_depth_hold_their_shape() {
        let cells: Vec<Value> = (0..12).map(|i| json!({ "star": 1 + i / 4, "items": i % 4, "games": 30, "avgPlace": 4.0, "top4Rate": 0.5 })).collect();
        let unit = json!({ "grid": cells, "teammates": [{ "id": "M", "name": "M", "cost": 1, "games": 300, "placeChange": -0.4 }],
            "recent": { "games": 5, "avgPlace": 4.0, "frequency": 0.1 } });
        let played = |b: &Value| b["grid"].as_array().unwrap().iter().filter(|c| c["games"] != json!(0)).count();
        let mut empty = unit.clone();
        stress("empty", "/api/units/X", &mut empty);
        assert_eq!((empty["grid"].as_array().unwrap().len(), played(&empty)), (12, 0));
        assert_eq!(empty["teammates"], json!([]));
        let mut one = unit.clone();
        stress("one", "/api/units/X", &mut one);
        assert_eq!(played(&one), 1);
        let mut worst = unit.clone();
        stress("worst", "/api/units/X", &mut worst);
        assert_eq!((played(&worst), worst["grid"][0]["games"].clone()), (8, json!(12_345_678)));
        let mut many = unit.clone();
        stress("many", "/api/units/X", &mut many);
        assert_eq!(many["teammates"].as_array().unwrap().len(), 10);

        let tr = json!({ "levels": [{ "id": "T|1", "level": 1, "units": 2, "games": 9, "avgPlace": 4.0,
            "set": { "units": [{ "id": "U", "name": "U", "cost": 1 }], "games": 5 } }] });
        let mut worst = tr.clone();
        stress("worst", "/api/traits/T", &mut worst);
        let levels = worst["levels"].as_array().unwrap();
        assert_eq!((levels.len(), levels[0]["set"]["units"].as_array().unwrap().len()), (8, 10));
        assert_eq!((levels[1]["set"].clone(), levels[0]["games"].clone()), (Value::Null, json!(12_345_678)));
        let mut empty = tr;
        stress("empty", "/api/traits/T", &mut empty);
        assert_eq!(empty["levels"], json!([]));
    }
}
