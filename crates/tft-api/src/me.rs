//! Your own account (`src/server/me.ts`): rank, recent placements and match history. The only routes that call
//! Riot while you browse. The account comes from the app (saved in the database) or, failing that, RIOT_ID and
//! MY_PLATFORM in .env. Bodies, status codes, error wording and what is stored match the TypeScript API.
//!
//! Riot errors use the TypeScript wording where the TypeScript API has its own: a missing key reads
//! "RIOT_API_KEY is not set. ..." as there. A key Riot rejects is reported at once (as the TypeScript client does,
//! with no activation wait) in the E4 wording of `tft_riot::KeyProblem`, which says which fix applies; the
//! TypeScript sentence names the status code and host instead.

use crate::describe::{Kind, describe};
use crate::js::{encode_component, num};
use crate::jsval::{JsValue, js_sort, js_string, nullish, obj, parse, strict_eq, to_number};
use crate::routes::Obj;
use anyhow::{Result, anyhow};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock, Mutex};
use tft_core::cluster::rarity_cost;
use tft_core::config::{Config, platform_route};
use tft_core::patches::Calendar;
use tft_riot::{KeyProblem, RiotClient, RiotError};
use tokio::sync::watch;

const ACCOUNT_KEY: &str = "account";
const STALE_MS: f64 = 5.0 * 60_000.0;

/// The characters JavaScript's `\s` and `trim()` treat as white space.
pub fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// `riotIdFromEnv()` in `src/config.ts`: dotenv reads `RIOT_ID=Name#TAG` as "Name", so when the tag is missing the
/// line is read from .env directly.
pub fn riot_id_from_env(parsed: &str, dot_env: Option<&str>) -> String {
    let parsed = js_trim(parsed).to_string();
    let Some(text) = dot_env.filter(|_| !parsed.contains('#')) else { return parsed };
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        let s = r"[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
        Regex::new(&format!(r"(?m)^{s}*RIOT_ID{s}*={s}*([^\n\r\u{{2028}}\u{{2029}}]*?){s}*$")).unwrap()
    });
    let line = LINE.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap_or("");
    // .replace(/^(["'])(.*)\1$/, '$2')
    let unquoted = match line.chars().next() {
        Some(q @ ('"' | '\'')) if line.chars().count() >= 2 && line.ends_with(q) => &line[1..line.len() - 1],
        _ => line,
    };
    let raw = js_trim(unquoted);
    if raw.contains('#') { raw.to_string() } else { parsed }
}

struct Run {
    refreshing: Option<watch::Receiver<bool>>,
    last_error: Option<String>,
}

pub struct Me {
    db: Arc<Mutex<Connection>>,
    riot: RiotClient,
    env_riot_id: String,
    /// `config.myPlatform || config.platforms[0]`; None where that is `undefined`.
    env_platform: Option<String>,
    demo: bool,
    calendar: Calendar,
    run: Mutex<Run>,
}

/// What the user typed into the app, or the .env account.
struct Account {
    riot_id: String,
    platform: Option<JsValue>,
}

fn kv(conn: &Connection, key: &str) -> Result<Option<JsValue>> {
    let raw: Option<String> =
        conn.query_row("SELECT value FROM kv WHERE key = ?", [key], |r| r.get(0)).optional()?;
    raw.map(|s| parse(&s)).transpose()
}
pub fn kv_set_js(conn: &Connection, key: &str, value: &JsValue) -> Result<()> {
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value.stringify()],
    )?;
    Ok(())
}

/// A field of the response, left out when it is `undefined`.
fn put(o: &mut Obj, key: &str, v: Option<&JsValue>) {
    if let Some(v) = v {
        o.insert(key.into(), v.to_json());
    }
}

/// Riot errors as the TypeScript API words them.
fn riot_message(e: RiotError) -> String {
    match e {
        RiotError::Key(KeyProblem::Missing) => "RIOT_API_KEY is not set. Copy .env.example to .env and add your key.".into(),
        other => other.to_string(),
    }
}

impl Me {
    pub fn new(db: Arc<Mutex<Connection>>, config: &Config, calendar: Calendar) -> Self {
        let dot_env = std::fs::read_to_string(".env").ok();
        let riot = RiotClient::new(&config.api_key, &config.api_base, config.rate_margin);
        // The TypeScript client reports a rejected key at once; no two-minute wait for a key that may be activating.
        riot.set_key_worked();
        let env_platform = Some(config.my_platform.clone()).filter(|p| !p.is_empty()).or_else(|| config.platforms.first().cloned());
        Me {
            db,
            riot,
            env_riot_id: riot_id_from_env(&config.riot_id, dot_env.as_deref()),
            env_platform,
            demo: std::env::var("DEMO").is_ok_and(|v| v == "1"),
            calendar,
            run: Mutex::new(Run { refreshing: None, last_error: None }),
        }
    }

    fn account(&self, conn: &Connection) -> Result<Account> {
        if let Some(saved) = kv(conn, ACCOUNT_KEY)? {
            if let Some(id) = saved.get("riotId").filter(|v| v.truthy()) {
                // A truthy non-string riotId would make `riotId.includes` throw.
                let riot_id = id.as_str().ok_or_else(|| anyhow!("riotId.includes is not a function"))?.to_string();
                return Ok(Account { riot_id, platform: saved.get("platform").cloned() });
            }
        }
        Ok(Account { riot_id: self.env_riot_id.clone(), platform: self.env_platform.clone().map(JsValue::Str) })
    }

    fn last_error(&self) -> Option<String> {
        self.run.lock().unwrap().last_error.clone()
    }

    async fn refresh(&self) -> Result<(), String> {
        let acct = { self.account(&self.db.lock().unwrap()).map_err(|e| e.to_string())? };
        let mut parts = acct.riot_id.split('#');
        let name = js_trim(parts.next().unwrap_or(""));
        let tag = js_trim(parts.next().unwrap_or(""));
        let platform = js_string(acct.platform.as_ref());
        let Some(route) = acct.platform.as_ref().and_then(|_| platform_route(&platform)) else {
            return Err(format!("\"{platform}\" is not a known server. Pick your server again."));
        };
        // Account lookups have no "sea" host; any of the three others returns the same account.
        let host = if route == "sea" { "asia" } else { route };
        let path = format!("/riot/account/v1/accounts/by-riot-id/{}/{}", encode_component(name), encode_component(tag));
        let found = self.riot.get::<JsValue>(host, &path).await.map_err(riot_message)?;
        let Some(found) = found.filter(JsValue::truthy) else {
            return Err(format!("No Riot account called {}. Check the spelling and the tag after #.", acct.riot_id));
        };
        let puuid = js_string(found.get("puuid"));

        let ranks = self.riot.get::<JsValue>(&platform, &format!("/tft/league/v1/by-puuid/{puuid}")).await.map_err(riot_message)?;
        let ranked = ranks
            .as_ref()
            .and_then(JsValue::as_array)
            .and_then(|xs| xs.iter().find(|e| e.get("queueType").and_then(JsValue::as_str) == Some("RANKED_TFT")))
            .cloned();
        let ids = self.riot.match_ids(route, &puuid, None, 20).await.map_err(riot_message)?;
        for id in ids {
            let have = {
                let conn = self.db.lock().unwrap();
                conn.query_row("SELECT 1 FROM my_matches WHERE match_id = ?", [&id], |_| Ok(())).optional().map_err(|e| e.to_string())?
            };
            if have.is_some() {
                continue;
            }
            let m = self.riot.get::<JsValue>(route, &format!("/tft/match/v1/matches/{id}")).await.map_err(riot_message)?;
            if let Some(m) = m.filter(JsValue::truthy) {
                let when = m.get("info").and_then(|i| i.get("game_datetime")).map(JsValue::to_number);
                let conn = self.db.lock().unwrap();
                conn.execute(
                    "INSERT OR REPLACE INTO my_matches (match_id, game_datetime, json) VALUES (?,?,?)",
                    params![id, when.filter(|w| !w.is_nan()), m.stringify()],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let rank = |k: &str| Some(nullish(ranked.as_ref().and_then(|r| r.get(k))).cloned().unwrap_or(JsValue::Null));
        let me = obj(vec![
            ("riotId", Some(JsValue::Str(acct.riot_id.clone()))),
            ("puuid", found.get("puuid").cloned()),
            ("name", found.get("gameName").cloned()),
            ("tag", found.get("tagLine").cloned()),
            ("platform", acct.platform.clone()),
            ("tier", rank("tier")),
            ("division", rank("rank")),
            ("lp", rank("leaguePoints")),
            ("wins", rank("wins")),
            ("losses", rank("losses")),
            ("fetchedAt", Some(JsValue::Num(now_ms()))),
        ]);
        kv_set_js(&self.db.lock().unwrap(), "me", &me).map_err(|e| e.to_string())
    }

    /// Starts a refresh, or joins the one already running. The receiver turns true when it has finished.
    fn start_refresh(self: &Arc<Self>) -> watch::Receiver<bool> {
        if self.demo {
            // Demo data: there is no real account to ask Riot about.
            return watch::channel(true).1;
        }
        let mut run = self.run.lock().unwrap();
        if let Some(rx) = &run.refreshing {
            return rx.clone();
        }
        let (tx, rx) = watch::channel(false);
        run.refreshing = Some(rx.clone());
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let result = me.refresh().await;
            {
                let mut run = me.run.lock().unwrap();
                run.last_error = result.err();
                run.refreshing = None;
            }
            let _ = tx.send(true);
        });
        rx
    }

    async fn refreshed(self: &Arc<Self>) {
        let _ = self.start_refresh().wait_for(|done| *done).await;
    }

    /// The stored matches of this account, newest first.
    fn my_matches(&self, conn: &Connection, puuid: Option<&JsValue>, limit: i64) -> Result<Vec<JsValue>> {
        let mut stmt = conn.prepare_cached("SELECT json FROM my_matches ORDER BY game_datetime DESC LIMIT ?")?;
        let texts = stmt.query_map([limit], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for t in texts {
            let m = parse(&t)?;
            let mine = participants(&m)?.iter().any(|p| strict_eq(p.get("puuid"), puuid));
            if mine {
                out.push(m);
            }
        }
        Ok(out)
    }

    // ---- Routes --------------------------------------------------------------------------------

    /// GET /api/me
    pub async fn get(self: &Arc<Self>) -> Result<Obj> {
        let acct = self.account(&self.db.lock().unwrap())?;
        let riot_id = JsValue::Str(acct.riot_id.clone());
        let mut base = Map::new();
        base.insert("configured".into(), json!(acct.riot_id.contains('#')));
        base.insert("riotId".into(), json!(acct.riot_id));
        if !acct.riot_id.contains('#') {
            put(&mut base, "platform", acct.platform.as_ref());
            return Ok(base);
        }
        let current = |conn: &Connection| -> Result<Option<JsValue>> {
            Ok(kv(conn, "me")?.filter(|m| m.truthy() && strict_eq(m.get("riotId"), Some(&riot_id))))
        };
        let me = current(&self.db.lock().unwrap())?;
        match &me {
            None => self.refreshed().await,
            Some(m) if now_ms() - to_number(m.get("fetchedAt")) > STALE_MS => drop(self.start_refresh()),
            Some(_) => {}
        }
        let conn = self.db.lock().unwrap();
        let Some(me) = current(&conn)? else {
            put(&mut base, "platform", acct.platform.as_ref());
            base.insert("error".into(), json!(self.last_error().unwrap_or_else(|| "Could not load your account.".into())));
            return Ok(base);
        };
        let puuid = me.get("puuid");
        let mut recent = Vec::new();
        for m in self.my_matches(&conn, puuid, 10)? {
            let p = participants(&m)?.iter().find(|p| strict_eq(p.get("puuid"), puuid)).cloned();
            recent.push(p.and_then(|p| p.get("placement").cloned()));
        }
        let (refreshing, error) = {
            let run = self.run.lock().unwrap();
            (run.refreshing.is_some(), run.last_error.clone())
        };
        base.insert("error".into(), json!(error));
        base.insert("refreshing".into(), json!(refreshing));
        for k in ["name", "tag", "platform", "tier", "division", "lp", "wins", "losses", "fetchedAt"] {
            put(&mut base, k, me.get(k));
        }
        let avg = if recent.is_empty() {
            Value::Null
        } else {
            num(recent.iter().map(|p| to_number(p.as_ref())).sum::<f64>() / recent.len() as f64)
        };
        base.insert("recent".into(), Value::Array(recent.iter().map(|p| p.as_ref().map(JsValue::to_json).unwrap_or(Value::Null)).collect()));
        base.insert("avgPlace".into(), avg);
        Ok(base)
    }

    /// POST /api/me/account: { riotId: "Name#TAG", platform: "na1" }. Checks the account with Riot straight away.
    /// Err(()) where the TypeScript handler throws (a body of `null`).
    pub async fn set_account(self: &Arc<Self>, body: Option<JsValue>) -> Result<(u16, Obj)> {
        let body = body.unwrap_or(JsValue::Obj(Vec::new()));
        if body == JsValue::Null {
            return Err(anyhow!("Cannot read properties of null (reading 'riotId')"));
        }
        let text = |k: &str| nullish(body.get(k)).map(JsValue::to_js_string).unwrap_or_default();
        let riot_id = normalise_riot_id(js_trim(&text("riotId")));
        let platform = js_trim(&text("platform")).to_lowercase();
        let fail = |msg: &str| Ok((400, json!({ "ok": false, "error": msg }).as_object().unwrap().clone()));
        if !valid_riot_id(&riot_id) {
            return fail("Write it as Name#TAG, the way the client shows it.");
        }
        if platform_route(&platform).is_none() {
            return fail("Pick the server you play on.");
        }
        {
            let conn = self.db.lock().unwrap();
            let acct = obj(vec![("riotId", Some(JsValue::Str(riot_id.clone()))), ("platform", Some(JsValue::Str(platform)))]);
            kv_set_js(&conn, ACCOUNT_KEY, &acct)?;
            let me = kv(&conn, "me")?;
            if !strict_eq(me.as_ref().and_then(|m| m.get("riotId")), Some(&JsValue::Str(riot_id))) {
                conn.execute("DELETE FROM my_matches", [])?; // games of the previous account
            }
        }
        self.refreshed().await;
        Ok((200, self.outcome()))
    }

    fn outcome(&self) -> Obj {
        let error = self.last_error();
        let ok = error.as_deref().is_none_or(str::is_empty);
        json!({ "ok": ok, "error": error }).as_object().unwrap().clone()
    }

    /// POST /api/me/refresh
    pub async fn refresh_now(self: &Arc<Self>) -> Result<Obj> {
        let acct = self.account(&self.db.lock().unwrap())?;
        if !acct.riot_id.contains('#') {
            return Ok(json!({ "configured": false }).as_object().unwrap().clone());
        }
        self.refreshed().await;
        Ok(self.outcome())
    }

    /// GET /api/me/matches
    pub fn matches(&self) -> Result<Obj> {
        let conn = self.db.lock().unwrap();
        let configured = self.account(&conn)?.riot_id.contains('#');
        let me = kv(&conn, "me")?.filter(JsValue::truthy);
        let Some(me) = me.filter(|_| configured) else {
            return Ok(json!({ "configured": configured, "matches": [] }).as_object().unwrap().clone());
        };
        let puuid = me.get("puuid");
        let mut out = Vec::new();
        for m in self.my_matches(&conn, puuid, 40)? {
            out.push(self.match_summary(&conn, &m, puuid)?);
        }
        Ok(json!({ "configured": true, "matches": out }).as_object().unwrap().clone())
    }

    fn match_summary(&self, conn: &Connection, m: &JsValue, puuid: Option<&JsValue>) -> Result<Value> {
        let info = m.get("info");
        let field = |k: &str| info.and_then(|i| i.get(k));
        let mut lobby: Vec<(f64, Obj)> = Vec::new();
        for p in participants(m)? {
            let mut o = Map::new();
            o.insert("me".into(), json!(strict_eq(p.get("puuid"), puuid)));
            let name = p.get("riotIdGameName").filter(|n| n.truthy()).map(|n| {
                let tag = nullish(p.get("riotIdTagline")).map(JsValue::to_js_string).unwrap_or_default();
                json!(format!("{}#{tag}", n.to_js_string()))
            });
            o.insert("name".into(), name.unwrap_or(Value::Null));
            put(&mut o, "placement", p.get("placement"));
            put(&mut o, "level", p.get("level"));
            put(&mut o, "damage", p.get("total_damage_to_players"));
            let mut units = Vec::new();
            for u in nullish(p.get("units")).map(list).transpose()?.unwrap_or_default() {
                let mut d = describe(conn, Kind::Unit, &js_string(u.get("character_id")), false)?;
                if d.get("cost").is_none_or(Value::is_null) {
                    // rarityCost: a lookup by property name, so 2 and "2" both read as rarity 2.
                    let rarity = nullish(u.get("rarity")).map(JsValue::to_js_string);
                    let rarity = rarity.and_then(|r| ["0", "1", "2", "4", "6"].contains(&r.as_str()).then(|| r.parse::<i64>().ok()).flatten());
                    d.insert("cost".into(), json!(rarity_cost(rarity)));
                }
                put(&mut d, "star", u.get("tier"));
                let mut items = Vec::new();
                for i in nullish(u.get("itemNames")).map(list).transpose()?.unwrap_or_default() {
                    items.push(Value::Object(describe(conn, Kind::Item, &i.to_js_string(), false)?));
                }
                d.insert("items".into(), Value::Array(items));
                units.push(Value::Object(d));
            }
            o.insert("units".into(), Value::Array(units));
            let mut traits: Vec<&JsValue> = nullish(p.get("traits"))
                .map(list)
                .transpose()?
                .unwrap_or_default()
                .iter()
                .filter(|t| to_number(t.get("tier_current")) > 0.0)
                .collect();
            js_sort(&mut traits, |a, b| to_number(b.get("num_units")) - to_number(a.get("num_units")));
            let mut ts = Vec::new();
            for t in traits {
                let mut d = describe(conn, Kind::Trait, &js_string(t.get("name")), false)?;
                put(&mut d, "units", t.get("num_units"));
                put(&mut d, "style", t.get("style"));
                ts.push(Value::Object(d));
            }
            o.insert("traits".into(), Value::Array(ts));
            lobby.push((to_number(p.get("placement")), o));
        }
        js_sort(&mut lobby, |a, b| a.0 - b.0);
        let lobby: Vec<Obj> = lobby.into_iter().map(|(_, o)| o).collect();

        let queue_id = nullish(field("queue_id")).or(nullish(field("queueId"))).map(JsValue::to_js_string).unwrap_or_else(|| "0".into());
        let queue = match queue_id.as_str() {
            "1100" => "Ranked",
            "1090" => "Normal",
            "1130" => "Hyper Roll",
            "1160" => "Double Up",
            _ => "Other mode",
        };
        let when = to_number(field("game_datetime"));
        let when = if when.is_nan() { i64::MIN } else { when.floor() as i64 };
        let patch = self.calendar.patch_for(field("game_version").and_then(JsValue::as_str), when);
        let mine = lobby.iter().find(|p| p.get("me") == Some(&Value::Bool(true))).cloned();
        let mut out = Map::new();
        put(&mut out, "id", m.get("metadata").and_then(|x| x.get("match_id")));
        put(&mut out, "date", field("game_datetime"));
        put(&mut out, "length", field("game_length"));
        out.insert("queue".into(), json!(queue));
        out.insert("patch".into(), json!(patch));
        out.insert("mine".into(), mine.map(Value::Object).unwrap_or(Value::Null));
        out.insert("lobby".into(), Value::Array(lobby.into_iter().map(Value::Object).collect()));
        Ok(Value::Object(out))
    }
}

/// `m.info.participants`, which the TypeScript code reads without a check.
fn participants(m: &JsValue) -> Result<&[JsValue]> {
    m.get("info").and_then(|i| i.get("participants")).and_then(JsValue::as_array).ok_or_else(|| anyhow!("match without participants"))
}
fn list(v: &JsValue) -> Result<&[JsValue]> {
    v.as_array().ok_or_else(|| anyhow!("not a list"))
}

/// `.replace(/\s*#\s*/, '#')`: the first # and the spaces around it.
pub fn normalise_riot_id(s: &str) -> String {
    let Some(at) = s.find('#') else { return s.to_string() };
    let left = s[..at].trim_end_matches(js_space);
    let right = s[at + 1..].trim_start_matches(js_space);
    format!("{left}#{right}")
}

/// `/^[^#]{3,16}#[^#]{2,5}$/`, counting UTF-16 code units as JavaScript does.
pub fn valid_riot_id(s: &str) -> bool {
    let Some((name, tag)) = s.split_once('#') else { return false };
    let len = |x: &str| x.encode_utf16().count();
    !tag.contains('#') && (3..=16).contains(&len(name)) && (2..=5).contains(&len(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn riot_id_from_env_reads_the_unquoted_line() {
        assert_eq!(riot_id_from_env(" Tester#NA1 ", None), "Tester#NA1");
        assert_eq!(riot_id_from_env("Tester", Some("A=1\nRIOT_ID=Tester#NA1\nB=2\n")), "Tester#NA1");
        assert_eq!(riot_id_from_env("", Some("RIOT_ID = \"Tester #NA1\" \r\n")), "Tester #NA1");
        assert_eq!(riot_id_from_env("", Some("RIOT_ID='Te#X'")), "Te#X");
        assert_eq!(riot_id_from_env("Name", Some("RIOT_ID=Name")), "Name");
        assert_eq!(riot_id_from_env("", Some("OTHER=1")), "");
        assert_eq!(riot_id_from_env("Has#Tag", Some("RIOT_ID=Other#X")), "Has#Tag");
    }

    #[test]
    fn checks_riot_ids_like_the_typescript_regex() {
        assert_eq!(normalise_riot_id("Tester  #\u{a0}NA1"), "Tester#NA1");
        assert_eq!(normalise_riot_id("a # b # c"), "a#b # c");
        for ok in ["Tes#NA", "Sixteen_chars_xx#ABCDE", "Té😀#NA1"] {
            assert!(valid_riot_id(ok), "{ok}");
        }
        for bad in ["NoTag", "Te#NA", "Tes#N", "Tes#ABCDEF", "Seventeen_chars_x#NA", "a#b#c", "Tes#NA#", "😀😀😀😀😀😀😀😀a#NA"] {
            assert!(!valid_riot_id(bad), "{bad}");
        }
    }
}
