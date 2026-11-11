//! Downloads from Community Dragon: the patch calendar, and the current set's names, costs, abilities, trait
//! breakpoints and icon paths (a port of `src/static/fetch.ts`, stored in the same `static_data` rows).
//!
//! [`refresh`] is what the supervisor runs every 3 hours: it notices a new patch folder, saves the new calendar,
//! dates matches to the right patch, and downloads the set data again when the live patch changed.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tft_core::patches::{Calendar, LISTING_URL, ListingEntry};
use tft_store::{Connection, kv_get, kv_set, load_calendar, params, relabel_unknown, save_calendar};

/// Where icon URLs point, whatever server the data was downloaded from (the app fetches icons from here).
pub const CDRAGON: &str = "https://raw.communitydragon.org";

/// `kv` key: the patch the stored set data was downloaded on. When the live patch moves on, it is downloaded again.
pub const STATIC_PATCH_KEY: &str = "static_patch";

/// Downloads the patch calendar. The caller saves it; on failure it keeps the saved one.
pub async fn fetch_calendar(http: &reqwest::Client) -> Result<Calendar> {
    fetch_calendar_from(http, LISTING_URL).await
}

/// [`fetch_calendar`] from another address (a test server).
pub async fn fetch_calendar_from(http: &reqwest::Client, url: &str) -> Result<Calendar> {
    let entries: Vec<ListingEntry> = http
        .get(url)
        .send()
        .await
        .context("downloading the patch list")?
        .error_for_status()?
        .json()
        .await
        .context("reading the patch list")?;
    let cal = Calendar::from_listing(&entries);
    if cal.is_empty() {
        bail!("no patches listed");
    }
    Ok(cal)
}

// ---- Set data, as src/static/fetch.ts -------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CdVar {
    pub name: String,
    #[serde(default)]
    pub value: Option<Vec<Option<f64>>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CdAbility {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub variables: Option<Vec<CdVar>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdUnit {
    pub api_name: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub cost: Value,
    pub traits: Option<Value>,
    pub role: Option<Value>,
    pub tile_icon: Option<String>,
    pub square_icon: Option<String>,
    pub icon: Option<String>,
    pub ability: Option<CdAbility>,
    pub stats: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdEffect {
    pub min_units: Value,
    pub style: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdTrait {
    pub api_name: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub effects: Option<Vec<CdEffect>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdItem {
    pub api_name: Option<String>,
    pub name: Option<String>,
    pub desc: Option<String>,
    pub icon: Option<String>,
    pub composition: Option<Vec<String>>,
    pub effects: Option<Map<String, Value>>,
    /// The traits an item gives (an emblem's trait), for PRD 3 U7.
    pub associated_traits: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CdSet {
    pub name: Option<String>,
    #[serde(default)]
    pub champions: Vec<CdUnit>,
    #[serde(default)]
    pub traits: Vec<CdTrait>,
}

/// `cdragon/tft/en_us.json`, the parts that are stored.
#[derive(Debug, Clone, Deserialize)]
pub struct CdData {
    #[serde(default)]
    pub items: Vec<CdItem>,
    #[serde(default)]
    pub sets: HashMap<String, CdSet>,
}

/// What [`store_static`] stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticSummary {
    pub set_number: String,
    pub set_name: String,
    pub units: usize,
    pub traits: usize,
    pub items: usize,
}

/// Downloads `<base>/<channel>/cdragon/tft/en_us.json` (base is [`CDRAGON`] outside tests).
pub async fn fetch_static(http: &reqwest::Client, base: &str, channel: &str) -> Result<CdData> {
    let url = format!("{base}/{channel}/cdragon/tft/en_us.json");
    http.get(&url)
        .send()
        .await
        .context("downloading the set data")?
        .error_for_status()?
        .json()
        .await
        .context("reading the set data")
}

/// "ASSETS/Characters/X/Icon.TFT_Set18.tex" -> a PNG URL on Community Dragon. `iconUrl` in fetch.ts.
pub fn icon_url(path: Option<&str>, channel: &str) -> Option<String> {
    let path = path.filter(|p| !p.is_empty())?;
    static EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(tex|dds)$").unwrap());
    Some(format!("{CDRAGON}/{channel}/game/{}", EXT.replace(&path.to_lowercase(), ".png")))
}

/// Sorts an item into the groups the Items page filters by. `itemType` in fetch.ts.
pub fn item_type(id: &str, composition: &[String]) -> &'static str {
    static RADIANT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)radiant").unwrap());
    static ARTIFACT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)artifact|ornn").unwrap());
    static EMBLEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)emblem").unwrap());
    static SUPPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)support").unwrap());
    static COMPONENTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^TFT_Item_(BFSword|RecurveBow|NeedlesslyLargeRod|TearOfTheGoddess|ChainVest|NegatronCloak|GiantsBelt|SparringGloves|Spatula|FryingPan)$").unwrap()
    });
    if RADIANT.is_match(id) {
        "radiant"
    } else if ARTIFACT.is_match(id) {
        "artifact"
    } else if EMBLEM.is_match(id) {
        "emblem"
    } else if SUPPORT.is_match(id) {
        "support"
    } else if COMPONENTS.is_match(id) {
        "component"
    } else if composition.len() == 2 {
        "craftable"
    } else {
        "other"
    }
}

/// A number as JavaScript's `String(n)` writes it, for the values this file formats.
fn js_number(n: f64) -> String {
    if n == 0.0 { "0".into() } else { format!("{n}") }
}

/// Ability text arrives with markup and @Variable@ placeholders. Makes it readable. `cleanText` in fetch.ts.
pub fn clean_text(desc: Option<&str>, variables: &[CdVar]) -> String {
    let Some(desc) = desc.filter(|d| !d.is_empty()) else { return String::new() };
    static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
    static VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@([A-Za-z0-9_.:]+?)(\*100)?@").unwrap());
    static ICON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%i:[^%]+%").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").unwrap());
    static EMPTY_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" ?\( ?\)").unwrap());
    static GAPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

    let by_name: HashMap<String, &Option<Vec<Option<f64>>>> =
        variables.iter().map(|v| (v.name.to_lowercase(), &v.value)).collect();
    let fmt = |n: f64| js_number(js_round(n * 100.0) / 100.0);

    let s = BR.replace_all(desc, "\n");
    let s = TAG.replace_all(&s, "");
    let s = VAR.replace_all(&s, |c: &regex::Captures| {
        let raw = c[1].to_lowercase();
        let pct = c.get(2).is_some();
        let value = by_name.get(&raw).or_else(|| by_name.get(raw.strip_prefix("modified").unwrap_or(&raw)));
        let Some(Some(value)) = value else { return "…".to_string() };
        // Index 1 to 3 are the one-, two- and three-star values.
        let stars: Vec<String> = (1..=3)
            .filter_map(|i| value.get(i).copied().flatten())
            .map(|v| fmt(if pct { v * 100.0 } else { v }))
            .collect();
        let mut distinct = stars.clone();
        distinct.sort();
        distinct.dedup();
        if distinct.len() == 1 { stars[0].clone() } else { stars.join("/") }
    });
    let s = ICON.replace_all(&s, "");
    let s = s.replace("&nbsp;", " ");
    let s = SPACES.replace_all(&s, " ");
    let s = EMPTY_PARENS.replace_all(&s, "");
    let s = GAPS.replace_all(&s, "\n\n");
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}').to_string()
}

/// `Math.round`: halves round up, towards positive infinity.
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

/// Replaces the stored set data with the newest set in `data`. `storeStatic` in fetch.ts.
pub fn store_static(conn: &mut Connection, data: &CdData, channel: &str) -> Result<StaticSummary> {
    let set_number = data
        .sets
        .keys()
        .filter_map(|k| k.trim().parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| (n, k)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(n, _)| js_number(n));
    let Some(set_number) = set_number else { bail!("the set data lists no sets") };
    let Some(set) = data.sets.get(&set_number) else { bail!("set {set_number} is missing from the set data") };

    let tx = conn.transaction()?;
    {
        tx.execute("DELETE FROM static_data", [])?;
        let mut ins = tx.prepare("INSERT OR REPLACE INTO static_data (kind, id, name, icon, data) VALUES (?,?,?,?,?)")?;
        let named = |id: &Option<String>, name: &Option<String>| match (id, name) {
            (Some(i), Some(n)) if !i.is_empty() && !n.is_empty() => Some((i.clone(), n.clone())),
            _ => None,
        };
        for u in &set.champions {
            let Some((id, name)) = named(&u.api_name, &u.name) else { continue };
            let icon = icon_url(u.tile_icon.as_deref().or(u.square_icon.as_deref()).or(u.icon.as_deref()), channel);
            let ability = match &u.ability {
                Some(a) if a.name.as_deref().is_some_and(|n| !n.is_empty()) => json!({
                    "name": a.name,
                    "text": clean_text(a.desc.as_deref(), a.variables.as_deref().unwrap_or_default()),
                }),
                _ => Value::Null,
            };
            let data = json!({
                "cost": u.cost,
                "traits": u.traits.clone().unwrap_or_else(|| json!([])),
                "role": u.role.clone().unwrap_or(Value::Null),
                "ability": ability,
                "stats": u.stats.clone().unwrap_or(Value::Null),
            });
            ins.execute(params!["unit", id, name, icon, data.to_string()])?;
        }
        for t in &set.traits {
            let Some((id, name)) = named(&t.api_name, &t.name) else { continue };
            let breakpoints: Vec<Value> = t
                .effects
                .iter()
                .flatten()
                .map(|e| json!({ "units": e.min_units, "style": e.style }))
                .collect();
            let data = json!({ "breakpoints": breakpoints });
            ins.execute(params!["trait", id, name, icon_url(t.icon.as_deref(), channel), data.to_string()])?;
        }
        for i in &data.items {
            let Some((id, name)) = named(&i.api_name, &i.name) else { continue };
            let composition = i.composition.clone().unwrap_or_default();
            let vars: Vec<CdVar> = i
                .effects
                .iter()
                .flatten()
                .map(|(name, v)| CdVar { name: name.clone(), value: Some(vec![v.as_f64(); 4]) })
                .collect();
            let mut data = json!({
                "composition": composition,
                "type": item_type(&id, &composition),
                "text": clean_text(i.desc.as_deref(), &vars),
            });
            // The trait an emblem gives (PRD 3 U7). Only written when there is one, so other items are unchanged.
            if let Some(t) = i.associated_traits.iter().flatten().find(|t| !t.is_empty()) {
                data["trait"] = json!(t);
            }
            ins.execute(params!["item", id, name, icon_url(i.icon.as_deref(), channel), data.to_string()])?;
        }
    }
    tx.commit()?;
    Ok(StaticSummary {
        set_name: set.name.clone().unwrap_or_default(),
        set_number,
        units: set.champions.len(),
        traits: set.traits.len(),
        items: data.items.len(),
    })
}

// ---- The 3-hourly refresh -------------------------------------------------------------------------------------

/// Where [`refresh`] downloads from. [`Source::live`] outside tests.
#[derive(Debug, Clone)]
pub struct Source {
    pub http: reqwest::Client,
    /// Community Dragon's address, without a trailing slash. The calendar is `<base>/json/`.
    pub base: String,
}

impl Source {
    pub fn live() -> Self {
        Source { http: crate::client::http_client(), base: CDRAGON.to_string() }
    }
}

/// What one [`refresh`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// A patch folder appeared that the saved calendar did not have (the newest one, if several).
    pub new_patch: Option<String>,
    /// The patch the set data now belongs to, when it was downloaded again in this refresh.
    pub static_set: Option<StaticSummary>,
    /// Why the set data could not be downloaded. The next refresh tries again.
    pub static_error: Option<String>,
    /// Matches given a different patch: ones stored without a patch, and ones played after a new patch came out
    /// but dated to the previous one because the calendar did not have it yet.
    pub relabelled: usize,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Downloads the patch calendar and, when the live patch has moved on, the set data. Saves both, then dates
/// matches against the new calendar. Stats, stat tables and older patches are left as they are, so an older patch
/// stays selectable. If the calendar cannot be downloaded, nothing changes and the error is returned.
pub async fn refresh(conn: &mut Connection, src: &Source) -> Result<Refreshed> {
    let old = load_calendar(conn)?;
    let new = fetch_calendar_from(&src.http, &format!("{}/json/", src.base)).await?;
    let mut out = Refreshed::default();

    let known: Vec<&str> = old.0.iter().map(|p| p.patch.as_str()).collect();
    let added: Vec<_> = new.0.iter().filter(|p| !known.contains(&p.patch.as_str())).collect();
    out.new_patch = added.last().map(|p| p.patch.clone());
    save_calendar(conn, &new)?;

    // Patch day: matches played after the new folder appeared were dated to the previous patch, because the saved
    // calendar did not have it. Newest first, so each match ends up on the latest patch released before it.
    if let Some(prev) = old.0.last() {
        let tx = conn.transaction()?;
        for p in added.iter().rev().filter(|p| p.since > prev.since) {
            out.relabelled += tx.execute(
                "UPDATE matches SET patch = ?1 WHERE patch = ?2 AND game_datetime >= ?3",
                params![p.patch, prev.patch, p.since],
            )?;
        }
        tx.commit()?;
    }
    out.relabelled += relabel_unknown(conn, &new)?;

    let live = new.patch_at(now_ms());
    let have: Option<String> = kv_get(conn, STATIC_PATCH_KEY)?;
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM static_data", [], |r| r.get(0))?;
    if have.as_deref() != Some(live.as_str()) || rows == 0 {
        let stored = match fetch_static(&src.http, &src.base, "latest").await {
            Ok(data) => store_static(conn, &data, "latest"),
            Err(e) => Err(e),
        };
        match stored {
            Ok(summary) => {
                kv_set(conn, STATIC_PATCH_KEY, &live)?;
                out.static_set = Some(summary);
            }
            Err(e) => out.static_error = Some(format!("{e:#}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tft_core::patches::PatchStart;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A fake Community Dragon: answers each path with what `files` holds for it (404 otherwise), and records
    /// every path asked for. Tests change `files` between refreshes.
    #[derive(Clone, Default)]
    struct FakeCdragon {
        files: Arc<Mutex<HashMap<String, (u16, String)>>>,
        hits: Arc<Mutex<Vec<String>>>,
    }

    impl FakeCdragon {
        async fn start(&self) -> Source {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let me = self.clone();
            tokio::spawn(async move {
                loop {
                    let (mut sock, _) = listener.accept().await.unwrap();
                    let mut req = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => req.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let path = String::from_utf8_lossy(&req).split(' ').nth(1).unwrap_or_default().to_string();
                    me.hits.lock().unwrap().push(path.clone());
                    let (status, body) = me.files.lock().unwrap().get(&path).cloned().unwrap_or((404, String::new()));
                    let answer = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(answer.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            });
            crate::client::init_tls();
            let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
            Source { http, base: format!("http://127.0.0.1:{port}") }
        }
        fn put(&self, path: &str, status: u16, body: String) {
            self.files.lock().unwrap().insert(path.into(), (status, body));
        }
        fn hits(&self, path: &str) -> usize {
            self.hits.lock().unwrap().iter().filter(|p| *p == path).count()
        }
    }

    const LISTING: &str = "/json/";
    const SET_DATA: &str = "/latest/cdragon/tft/en_us.json";
    /// 16.21 came out 21 Oct 2026 07:15:23 GMT, 16.22 on 4 Nov 2026 07:47:21 GMT.
    const P21: i64 = 1_792_566_923_000;
    const P22: i64 = 1_793_778_441_000;
    const DAY: i64 = 86_400_000;

    fn listing(with_22: bool) -> String {
        let mut l = vec![
            json!({ "name": "latest", "mtime": "Wed, 04 Nov 2026 07:47:21 GMT" }),
            json!({ "name": "16.21", "mtime": "Wed, 21 Oct 2026 07:15:23 GMT" }),
            json!({ "name": "pbe", "mtime": "Thu, 27 Feb 2025 06:58:53 GMT" }),
        ];
        if with_22 {
            l.push(json!({ "name": "16.22", "mtime": "Wed, 04 Nov 2026 07:47:21 GMT" }));
        }
        Value::Array(l).to_string()
    }

    /// Set data with one unit, one trait and one item, named after `tag`.
    fn set_data(tag: &str) -> String {
        json!({
            "items": [ { "apiName": format!("TFT_Item_{tag}"), "name": format!("Item {tag}"), "desc": "Gain @AD@ AD.",
                         "icon": format!("ASSETS/Maps/TFT/Icons/Items/{tag}.TFT_Set16.tex"),
                         "composition": ["TFT_Item_BFSword", "TFT_Item_ChainVest"], "effects": { "AD": 15 } } ],
            "sets": {
                "15": { "name": "Old", "champions": [], "traits": [] },
                "16": {
                    "name": format!("Set {tag}"),
                    "champions": [ { "apiName": format!("TFT16_{tag}"), "name": format!("Unit {tag}"), "cost": 3,
                                     "traits": ["Bruiser"], "role": null,
                                     "tileIcon": format!("ASSETS/Characters/TFT16_{tag}/HUD/{tag}_Square.TFT_Set16.tex"),
                                     "ability": { "name": "Hit", "desc": "Deal @Damage@ damage.",
                                                  "variables": [ { "name": "Damage", "value": [0, 100, 150, 900] } ] },
                                     "stats": { "hp": 800, "range": 1 } } ],
                    "traits": [ { "apiName": "TFT16_Bruiser", "name": "Bruiser", "icon": null,
                                  "effects": [ { "minUnits": 2, "style": 1 }, { "minUnits": 4, "style": 3 } ] } ]
                }
            }
        })
        .to_string()
    }

    fn temp_db(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("hertz-cdragon-{}-{name}.db", std::process::id()));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
        p
    }

    /// A database as it is the day before 16.22: calendar and set data for 16.21, some matches.
    fn before_patch_day(name: &str) -> Connection {
        let mut conn = tft_store::open(&temp_db(name)).unwrap();
        save_calendar(&conn, &Calendar(vec![PatchStart { patch: "16.21".into(), since: P21 }])).unwrap();
        store_static(&mut conn, &serde_json::from_str(&set_data("Old")).unwrap(), "latest").unwrap();
        kv_set(&conn, STATIC_PATCH_KEY, &"16.21").unwrap();
        for (id, patch, at) in [
            ("NA1_old", "16.21", P22 - DAY),       // really 16.21
            ("NA1_patchday", "16.21", P22 + DAY),  // patch day: dated with a calendar that had no 16.22 yet
            ("NA1_unknown", "unknown", P22 + DAY), // stored before any calendar
            ("NA1_unknown_old", "unknown", P22 - DAY),
        ] {
            conn.execute(
                "INSERT INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank) VALUES (?1,'na1',?2,16,?3,2000,1)",
                params![id, patch, at],
            )
            .unwrap();
        }
        conn
    }

    fn patch_of(conn: &Connection, id: &str) -> String {
        conn.query_row("SELECT patch FROM matches WHERE match_id = ?", [id], |r| r.get(0)).unwrap()
    }

    fn static_ids(conn: &Connection) -> Vec<String> {
        let mut st = conn.prepare("SELECT kind || ':' || id FROM static_data ORDER BY kind, id").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().collect::<std::result::Result<_, tft_store::Error>>().unwrap()
    }

    fn patches_in_stats(conn: &Connection) -> Vec<String> {
        let mut st = conn.prepare("SELECT DISTINCT patch FROM stat_meta ORDER BY patch").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().collect::<std::result::Result<_, tft_store::Error>>().unwrap()
    }

    #[tokio::test]
    async fn a_new_patch_folder_updates_calendar_and_set_data_and_relabels_matches() {
        let cd = FakeCdragon::default();
        cd.put(LISTING, 200, listing(true));
        cd.put(SET_DATA, 200, set_data("New"));
        let src = cd.start().await;
        let mut conn = before_patch_day("new-patch");

        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!(r.new_patch.as_deref(), Some("16.22"));
        assert_eq!(r.static_error, None);
        assert_eq!(r.static_set.as_ref().map(|s| (s.set_number.as_str(), s.set_name.as_str())), Some(("16", "Set New")));

        // Calendar: both patches, saved.
        let cal = load_calendar(&conn).unwrap();
        assert_eq!(cal, Calendar(vec![
            PatchStart { patch: "16.21".into(), since: P21 },
            PatchStart { patch: "16.22".into(), since: P22 },
        ]));
        // Set data and icons: the new set's rows replace the old ones.
        assert_eq!(static_ids(&conn), ["item:TFT_Item_New", "trait:TFT16_Bruiser", "unit:TFT16_New"]);
        let icon: String = conn.query_row("SELECT icon FROM static_data WHERE kind='unit'", [], |r| r.get(0)).unwrap();
        assert_eq!(icon, "https://raw.communitydragon.org/latest/game/assets/characters/tft16_new/hud/new_square.tft_set16.png");
        assert_eq!(kv_get::<String>(&conn, STATIC_PATCH_KEY).unwrap().as_deref(), Some("16.22"));
        // Matches: patch-day and unknown ones move to 16.22; older ones stay on 16.21.
        assert_eq!(patch_of(&conn, "NA1_old"), "16.21");
        assert_eq!(patch_of(&conn, "NA1_patchday"), "16.22");
        assert_eq!(patch_of(&conn, "NA1_unknown"), "16.22");
        assert_eq!(patch_of(&conn, "NA1_unknown_old"), "16.21");
        assert_eq!(r.relabelled, 3);
    }

    #[tokio::test]
    async fn no_new_patch_means_no_set_data_download() {
        let cd = FakeCdragon::default();
        cd.put(LISTING, 200, listing(false));
        cd.put(SET_DATA, 200, set_data("New"));
        let src = cd.start().await;
        let mut conn = before_patch_day("same-patch");

        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!(r.new_patch, None);
        assert_eq!(r.static_set, None);
        assert_eq!(cd.hits(SET_DATA), 0);
        assert_eq!(static_ids(&conn), ["item:TFT_Item_Old", "trait:TFT16_Bruiser", "unit:TFT16_Old"]);
        assert_eq!(patch_of(&conn, "NA1_patchday"), "16.21", "16.21 is still the latest patch");
        assert_eq!(patch_of(&conn, "NA1_unknown"), "16.21");

        // Then the 16.22 folder appears.
        cd.put(LISTING, 200, listing(true));
        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!(r.new_patch.as_deref(), Some("16.22"));
        assert_eq!(cd.hits(SET_DATA), 1);
        assert_eq!(patch_of(&conn, "NA1_patchday"), "16.22");
        assert_eq!(patch_of(&conn, "NA1_unknown"), "16.22", "relabelled once the calendar has the new patch");
        // And again: nothing new.
        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!((r.new_patch, r.static_set, r.relabelled), (None, None, 0));
        assert_eq!(cd.hits(SET_DATA), 1);
    }

    #[tokio::test]
    async fn the_old_patch_stays_selectable() {
        let cd = FakeCdragon::default();
        cd.put(LISTING, 200, listing(true));
        cd.put(SET_DATA, 200, set_data("New"));
        let src = cd.start().await;
        let mut conn = before_patch_day("old-selectable");
        tft_stats::build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(patches_in_stats(&conn), ["16.21"]);
        let meta_before: i64 = conn.query_row("SELECT COUNT(*) FROM stat_meta WHERE patch = '16.21'", [], |r| r.get(0)).unwrap();

        refresh(&mut conn, &src).await.unwrap();
        assert_eq!(patches_in_stats(&conn), ["16.21"], "the refresh leaves stats alone");
        tft_stats::build_stats_with(&mut conn, 1, &mut |_| {}).unwrap(); // the newest patch only
        assert_eq!(patches_in_stats(&conn), ["16.21", "16.22"], "the patch list (as /api/stats reads it) has both");
        let meta_after: i64 = conn.query_row("SELECT COUNT(*) FROM stat_meta WHERE patch = '16.21'", [], |r| r.get(0)).unwrap();
        assert_eq!(meta_after, meta_before, "16.21's stats are still there");
        let cal = load_calendar(&conn).unwrap();
        assert!(cal.0.iter().any(|p| p.patch == "16.21"), "and the calendar still has it");
    }

    #[tokio::test]
    async fn a_failed_calendar_download_changes_nothing() {
        let cd = FakeCdragon::default();
        cd.put(LISTING, 500, String::new());
        cd.put(SET_DATA, 200, set_data("New"));
        let src = cd.start().await;
        let mut conn = before_patch_day("calendar-fails");
        assert!(refresh(&mut conn, &src).await.is_err());
        assert_eq!(load_calendar(&conn).unwrap().0.len(), 1);
        assert_eq!(cd.hits(SET_DATA), 0);
        assert_eq!(patch_of(&conn, "NA1_unknown"), "unknown");
    }

    #[tokio::test]
    async fn a_failed_set_data_download_is_tried_again_next_time() {
        let cd = FakeCdragon::default();
        cd.put(LISTING, 200, listing(true));
        cd.put(SET_DATA, 503, String::new());
        let src = cd.start().await;
        let mut conn = before_patch_day("static-fails");
        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!(r.new_patch.as_deref(), Some("16.22"));
        assert!(r.static_error.is_some());
        assert_eq!(static_ids(&conn), ["item:TFT_Item_Old", "trait:TFT16_Bruiser", "unit:TFT16_Old"], "old data kept");
        assert_eq!(load_calendar(&conn).unwrap().0.len(), 2, "the calendar still updates");

        cd.put(SET_DATA, 200, set_data("New"));
        let r = refresh(&mut conn, &src).await.unwrap();
        assert_eq!(r.new_patch, None);
        assert!(r.static_set.is_some(), "downloaded on the next refresh");
        assert_eq!(static_ids(&conn), ["item:TFT_Item_New", "trait:TFT16_Bruiser", "unit:TFT16_New"]);
    }

    #[test]
    fn stored_rows_are_the_typescript_ones() {
        // Same cases as test/e2e.ts, section 4.
        assert_eq!(
            clean_text(
                Some("Deal <b>@Damage@</b> (%i:scaleAD%) damage.<br>Stun for @Stun*100@%."),
                &[
                    CdVar { name: "Damage".into(), value: Some(vec![Some(0.0), Some(100.0), Some(150.0), Some(900.0)]) },
                    CdVar { name: "Stun".into(), value: Some(vec![Some(0.5); 4]) },
                ]
            ),
            "Deal 100/150/900 damage.\nStun for 50%."
        );
        assert_eq!(clean_text(Some("Gain @Missing@ and @ModifiedAD@."), &[CdVar { name: "AD".into(), value: Some(vec![Some(1.0 / 3.0); 4]) }]), "Gain … and 0.33.");
        let types: Vec<_> = ["TFT5_Item_BloodthirsterRadiant", "TFT_Item_Artifact_Fishbones", "TFT99_Item_VanguardEmblemItem", "TFT_Item_BFSword", "TFT_Item_X"]
            .iter()
            .map(|i| item_type(i, &[]))
            .collect();
        assert_eq!(types, ["radiant", "artifact", "emblem", "component", "other"]);
        assert_eq!(
            icon_url(Some("ASSETS/Characters/TFT99_Unit19/HUD/TFT99_Unit19_Square.TFT_Set99.tex"), "latest").as_deref(),
            Some("https://raw.communitydragon.org/latest/game/assets/characters/tft99_unit19/hud/tft99_unit19_square.tft_set99.png")
        );

        let mut conn = tft_store::open(&temp_db("rows")).unwrap();
        let s = store_static(&mut conn, &serde_json::from_str(&set_data("X")).unwrap(), "latest").unwrap();
        assert_eq!(s, StaticSummary { set_number: "16".into(), set_name: "Set X".into(), units: 1, traits: 1, items: 1 });
        let row = |kind: &str| -> (String, Option<String>, Value) {
            conn.query_row("SELECT name, icon, data FROM static_data WHERE kind = ?", [kind], |r| {
                Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&r.get::<_, String>(2)?).unwrap()))
            })
            .unwrap()
        };
        assert_eq!(row("unit").2, json!({ "cost": 3, "traits": ["Bruiser"], "role": null,
            "ability": { "name": "Hit", "text": "Deal 100/150/900 damage." }, "stats": { "hp": 800, "range": 1 } }));
        assert_eq!(row("trait"), ("Bruiser".into(), None, json!({ "breakpoints": [ { "units": 2, "style": 1 }, { "units": 4, "style": 3 } ] })));
        assert_eq!(row("item").2, json!({ "composition": ["TFT_Item_BFSword", "TFT_Item_ChainVest"], "type": "craftable", "text": "Gain 15 AD." }));
    }

    /// PRD 3 U7: an emblem's trait comes from `associatedTraits`; items without one store no trait.
    #[test]
    fn emblems_store_the_trait_they_give() {
        let mut data: Value = serde_json::from_str(&set_data("X")).unwrap();
        data["items"].as_array_mut().unwrap().extend([
            json!({ "apiName": "TFT16_Item_BruiserEmblemItem", "name": "Bruiser Emblem", "composition": ["TFT_Item_Spatula", "TFT_Item_GiantsBelt"],
                    "associatedTraits": ["TFT16_Bruiser"] }),
            json!({ "apiName": "TFT_Item_Y", "name": "Y", "associatedTraits": [] }),
        ]);
        let mut conn = tft_store::open(&temp_db("emblem")).unwrap();
        store_static(&mut conn, &serde_json::from_value(data).unwrap(), "latest").unwrap();
        let data = |id: &str| -> Value {
            serde_json::from_str(&conn.query_row("SELECT data FROM static_data WHERE id = ?", [id], |r| r.get::<_, String>(0)).unwrap()).unwrap()
        };
        assert_eq!(data("TFT16_Item_BruiserEmblemItem")["trait"], json!("TFT16_Bruiser"));
        assert_eq!(data("TFT16_Item_BruiserEmblemItem")["type"], json!("emblem"));
        assert!(data("TFT_Item_Y").get("trait").is_none());
        assert!(data("TFT_Item_X").get("trait").is_none());
    }
}
