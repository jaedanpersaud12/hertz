//! Collects ranked TFT matches into the local database. Stores what `src/crawler/crawl.ts` stores, with two
//! deliberate differences, both about stopping and starting again:
//!
//! - A server whose ladders were not read to the end (killed mid-seed) has its ladders read again on the next run
//!   (`seed_pending`). The TypeScript crawler reads ladders only when it knows no players for that server
//!   or is given `--reseed`.
//! - A match answer that was cut off on the way is fetched again, up to 3 times per run, then left queued for the
//!   next run. The TypeScript crawler stops on it.
//!
//! It is safe to stop and restart at any time: the queue and progress live in the database.
//!
//! Test only: `TFT_DIE_AT=<point>:<n>` aborts the process at an exact point between database writes, for
//! `scripts/resume.ts`. See [`die_at`]. It does nothing unless that variable is set.

use crate::cdragon::fetch_calendar;
use crate::client::{RiotClient, RiotError, http_client};
use anyhow::{Result, bail};
use futures::future::join_all;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tft_core::config::{Config, TIERS, platform_route, tier_name, tier_rank};
use tft_core::patches::Calendar;
use tft_core::riot::LeagueEntry;
use tft_store::{Connection, Ingest, kv_get, kv_set, load_calendar, params, save_calendar};

/// Look at the same player again after this long.
const RECRAWL_MS: i64 = 6 * 3600 * 1000;

/// Settings key: servers whose ladders were not read to the end (the crawler was stopped or killed).
const SEED_PENDING: &str = "seed_pending";

/// A match whose answer is cut off this many times in one run is left queued for the next run.
const CUT_OFF_LIMIT: u32 = 3;

#[derive(Debug, Clone, Default)]
pub struct CrawlOptions {
    /// Stop after storing this many matches.
    pub max: Option<u64>,
    /// Refresh the player list from the ladders first.
    pub reseed: bool,
    /// Stop when every known player is up to date.
    pub once: bool,
}

pub struct Crawler {
    config: Config,
    opts: CrawlOptions,
    riot: RiotClient,
    db: Mutex<Connection>,
    calendar: RwLock<Calendar>,
    stored: AtomicU64,
    stop: Arc<AtomicBool>,
    /// How often each match's answer was cut off in this run.
    cut_off: Mutex<HashMap<String, u32>>,
}

/// Test only, for `scripts/resume.ts`. With `TFT_DIE_AT=<point>:<n>` set, the nth time this run reaches `point`
/// it prints `TFT_DIE_AT <point>:<n> <what>` to stderr and aborts, so a kill lands exactly between two database
/// writes. Points: `after_ladder` (a ladder page is saved), `after_seed` (a server's ladders are all saved, before
/// it leaves `seed_pending`), `after_enqueue` (one player's match list is queued), `before_store` (a match is
/// downloaded, before its store), `after_store` (its store is committed, before its queue status is set).
/// Does nothing when the variable is not set.
fn die_at(point: &str, what: &str) {
    static PLAN: OnceLock<Option<(String, u64)>> = OnceLock::new();
    static HITS: AtomicU64 = AtomicU64::new(0);
    let plan = PLAN.get_or_init(|| {
        let v = std::env::var("TFT_DIE_AT").ok()?;
        let (p, n) = v.rsplit_once(':')?;
        Some((p.to_string(), n.parse().ok()?))
    });
    if let Some((p, n)) = plan
        && p == point
        && HITS.fetch_add(1, Ordering::SeqCst) + 1 == *n
    {
        eprintln!("TFT_DIE_AT {point}:{n} {what}");
        std::process::abort();
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

impl Crawler {
    pub fn new(config: Config, opts: CrawlOptions, conn: Connection, stop: Arc<AtomicBool>) -> Result<Arc<Self>> {
        let unknown: Vec<_> = config.platforms.iter().filter(|p| platform_route(p).is_none()).cloned().collect();
        if !unknown.is_empty() {
            bail!("Unknown server(s) in PLATFORMS: {}", unknown.join(", "));
        }
        let calendar = load_calendar(&conn)?;
        Ok(Arc::new(Crawler {
            riot: RiotClient::new(&config.api_key, &config.api_base, config.rate_margin),
            config,
            opts,
            db: Mutex::new(conn),
            calendar: RwLock::new(calendar),
            stored: AtomicU64::new(0),
            stop,
            cut_off: Mutex::new(HashMap::new()),
        }))
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn below_max(&self) -> bool {
        self.opts.max.is_none_or(|m| self.stored.load(Ordering::Relaxed) < m)
    }

    /// Sleeps in one-second steps so Ctrl+C is noticed quickly.
    async fn pause(&self, secs: u64) {
        for _ in 0..secs {
            if self.stopping() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Downloads the patch calendar and saves it. Keeps the saved one if the download fails.
    pub async fn refresh_calendar(&self) -> bool {
        match fetch_calendar(&http_client()).await {
            Ok(cal) => {
                let saved = save_calendar(&self.db.lock().unwrap(), &cal);
                *self.calendar.write().unwrap() = cal;
                saved.is_ok()
            }
            Err(_) => false,
        }
    }

    pub async fn run(self: &Arc<Self>) -> Result<()> {
        let got = self.refresh_calendar().await;
        if got {
            println!("Current patch: {}", self.calendar.read().unwrap().patch_at(now_ms()));
        } else {
            println!("Could not download the patch calendar. Matches will be labelled when the stats are next built.");
        }
        let me = Arc::clone(self);
        let calendar_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3 * 3600)).await;
                me.refresh_calendar().await;
            }
        });

        for platform in &self.config.platforms {
            let have: i64 = self.db.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM players WHERE platform = ?1",
                [platform],
                |r| r.get(0),
            )?;
            // A kill while reading the ladders leaves the platform in SEED_PENDING, so the next run reads them again.
            let pending: Vec<String> = kv_get(&self.db.lock().unwrap(), SEED_PENDING)?.unwrap_or_default();
            if have == 0 || self.opts.reseed || pending.contains(platform) {
                self.set_seed_pending(platform, true)?;
                println!("[{platform}] reading ladders...");
                self.seed(platform).await?;
                if !self.stopping() {
                    die_at("after_seed", platform);
                    self.set_seed_pending(platform, false)?;
                }
            }
        }

        let mut routes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        for p in &self.config.platforms {
            routes.entry(platform_route(p).unwrap()).or_default().push(p.clone());
        }

        let started = std::time::Instant::now();
        let me = Arc::clone(self);
        let report_task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.tick().await;
            loop {
                tick.tick().await;
                me.report(started);
            }
        });

        let summary: Vec<String> = routes.iter().map(|(r, ps)| format!("{r} ({})", ps.join(", "))).collect();
        println!("Crawling {}. Press Ctrl+C to stop.", summary.join(" | "));

        let results = join_all(routes.into_iter().map(|(route, platforms)| {
            let me = Arc::clone(self);
            async move { me.crawl_route(route, platforms).await }
        }))
        .await;
        report_task.abort();
        calendar_task.abort();
        for r in results {
            r?;
        }

        let total: i64 = self.db.lock().unwrap().query_row("SELECT COUNT(*) FROM matches", [], |r| r.get(0))?;
        println!(
            "Stopped. Stored {} matches this run. Database now holds {total} matches. Run the stats step to refresh the app.",
            self.stored.load(Ordering::Relaxed)
        );
        Ok(())
    }

    fn report(&self, started: std::time::Instant) {
        let stored = self.stored.load(Ordering::Relaxed);
        let per_hour = (stored as f64 / started.elapsed().as_secs_f64().max(1.0) * 3600.0).round();
        let totals = self.db.lock().unwrap().query_row(
            "SELECT (SELECT COUNT(*) FROM matches), (SELECT COUNT(*) FROM players), (SELECT COUNT(*) FROM match_queue WHERE status = 0)",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
        );
        if let Ok((m, p, q)) = totals {
            let c = &self.riot.counts;
            println!(
                "stored {stored} this run ({per_hour}/hour) | database: {m} matches, {p} players, {q} queued | requests ok {}, rate-limited {}, retried {}",
                c.ok.load(Ordering::Relaxed),
                c.limited.load(Ordering::Relaxed),
                c.retried.load(Ordering::Relaxed),
            );
        }
    }

    /// Adds or removes a platform from the list of servers whose ladders were not read to the end.
    fn set_seed_pending(&self, platform: &str, unfinished: bool) -> Result<()> {
        let db = self.db.lock().unwrap();
        let mut pending: Vec<String> = kv_get(&db, SEED_PENDING)?.unwrap_or_default();
        pending.retain(|p| p != platform);
        if unfinished {
            pending.push(platform.to_string());
        }
        if pending.is_empty() {
            db.execute("DELETE FROM kv WHERE key = ?1", [SEED_PENDING])?;
        } else {
            kv_set(&db, SEED_PENDING, &pending)?;
        }
        Ok(())
    }

    /// Fills the player list for one server from its ranked ladders.
    async fn seed(&self, platform: &str) -> Result<()> {
        let save = |entries: &[LeagueEntry], tier: &str| -> Result<usize> {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            let mut n = 0;
            {
                let mut up = tx.prepare_cached(
                    "INSERT INTO players (puuid, platform, tier, rank) VALUES (?,?,?,?)
                     ON CONFLICT(puuid) DO UPDATE SET tier = excluded.tier, rank = excluded.rank, platform = excluded.platform",
                )?;
                for e in entries {
                    if let Some(puuid) = &e.puuid {
                        up.execute(params![puuid, platform, tier, tier_rank(tier)])?;
                        n += 1;
                    }
                }
            }
            tx.commit()?;
            die_at("after_ladder", &format!("{platform}/{tier}"));
            Ok(n)
        };

        for tier in &self.config.tiers {
            if self.stopping() {
                return Ok(());
            }
            let (mut found, mut missing) = (0, 0);
            if ["CHALLENGER", "GRANDMASTER", "MASTER"].contains(&tier.as_str()) {
                let entries = self.riot.apex_league(platform, tier).await?;
                found = save(&entries, tier)?;
                missing = entries.len() - found;
            } else if TIERS.contains(&tier.as_str()) {
                for division in ["I", "II", "III", "IV"] {
                    for page in 1..=self.config.pages_per_division {
                        if self.stopping() {
                            break;
                        }
                        let entries = self.riot.league_page(platform, tier, division, page).await?;
                        if entries.is_empty() {
                            break;
                        }
                        let n = save(&entries, tier)?;
                        found += n;
                        missing += entries.len() - n;
                    }
                }
            }
            let note =
                if missing > 0 { format!(" ({missing} entries had no puuid and were skipped)") } else { String::new() };
            println!("[{platform}] {tier}: {found} players{note}");
        }
        Ok(())
    }

    /// One loop per routing group. Each group has its own rate limit, so they run side by side.
    async fn crawl_route(&self, route: &str, platforms: Vec<String>) -> Result<()> {
        let platforms_json = serde_json::to_string(&platforms)?;
        let mut finished = false;
        while !self.stopping() && !finished && self.below_max() {
            match self.step(route, &platforms_json).await {
                Ok(done) => finished = done,
                Err(e) => match e.downcast_ref::<RiotError>() {
                    // Nothing was marked as done, so the same players and matches are retried after the pause.
                    Some(RiotError::Unavailable(msg)) => {
                        println!("[{route}] {msg} Pausing this region for a minute.");
                        self.pause(60).await;
                    }
                    _ => return Err(e),
                },
            }
        }
        Ok(())
    }

    /// Queued matches this run still has to fetch (the ones held back after repeated cut-offs are left out).
    fn pending(&self, route: &str) -> Result<i64> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM match_queue WHERE route = ?1 AND status = 0
             AND match_id NOT IN (SELECT value FROM json_each(?2))",
            params![route, self.held_back()?],
            |r| r.get(0),
        )?)
    }

    /// Matches whose answer was cut off too often in this run, as a JSON list. They stay queued for the next run.
    fn held_back(&self) -> Result<String> {
        let cut_off = self.cut_off.lock().unwrap();
        let ids: Vec<&String> = cut_off.iter().filter(|(_, n)| **n >= CUT_OFF_LIMIT).map(|(id, _)| id).collect();
        Ok(serde_json::to_string(&ids)?)
    }

    /// One round: top up the queue, then fetch a batch of matches. Returns true when `--once` is satisfied.
    async fn step(&self, route: &str, platforms_json: &str) -> Result<bool> {
        if self.pending(route)? < 40 {
            let players: Vec<(String, String, i64, i64)> = {
                let db = self.db.lock().unwrap();
                let mut q = db.prepare_cached(
                    "SELECT puuid, platform, rank, last_crawled FROM players
                     WHERE platform IN (SELECT value FROM json_each(?1)) AND last_crawled < ?2
                     ORDER BY last_crawled ASC, rank DESC LIMIT ?3",
                )?;
                q.query_map(params![platforms_json, now_ms() - RECRAWL_MS, 8], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite_result::Result<_>>()?
            };
            if players.is_empty() && self.pending(route)? == 0 {
                if self.opts.once {
                    return Ok(true);
                }
                println!("[{route}] every known player is up to date. Checking again in 10 minutes.");
                self.pause(600).await;
                return Ok(false);
            }

            let since_floor = now_ms() / 1000 - self.config.days as i64 * 86400;
            let fetched = join_all(players.iter().map(|(puuid, _, _, last)| {
                let since = since_floor.max(last / 1000);
                self.riot.match_ids(route, puuid, Some(since), 30)
            }))
            .await;
            let mut first_err = None;
            {
                let mut db = self.db.lock().unwrap();
                for ((puuid, platform, rank, _), ids) in players.iter().zip(fetched) {
                    match ids {
                        Ok(ids) => {
                            let tx = db.transaction()?;
                            {
                                let mut enq = tx.prepare_cached(
                                    "INSERT OR IGNORE INTO match_queue (match_id, route, platform, seed_rank) VALUES (?,?,?,?)",
                                )?;
                                for id in &ids {
                                    enq.execute(params![id, route, platform, rank])?;
                                }
                                tx.execute("UPDATE players SET last_crawled = ?1 WHERE puuid = ?2", params![now_ms(), puuid])?;
                            }
                            tx.commit()?;
                            die_at("after_enqueue", puuid);
                        }
                        Err(e) => {
                            first_err.get_or_insert(e);
                        }
                    }
                }
            }
            if let Some(e) = first_err {
                return Err(e.into());
            }
        }

        let batch: Vec<(String, String, i64)> = {
            let db = self.db.lock().unwrap();
            let mut q = db.prepare_cached(
                "SELECT match_id, platform, seed_rank FROM match_queue WHERE route = ?1 AND status = 0
                 AND match_id NOT IN (SELECT value FROM json_each(?2)) LIMIT ?3",
            )?;
            q.query_map(params![route, self.held_back()?, 20], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite_result::Result<_>>()?
        };

        let fetched = join_all(batch.iter().map(|(id, _, _)| async move {
            if self.stopping() || !self.below_max() {
                return None;
            }
            Some(self.riot.get_match(route, id).await)
        }))
        .await;

        let set_status = |id: &str, status: i64| -> Result<()> {
            self.db.lock().unwrap().execute("UPDATE match_queue SET status = ?1 WHERE match_id = ?2", params![status, id])?;
            Ok(())
        };
        let mut first_err = None;
        for ((id, platform, seed_rank), got) in batch.iter().zip(fetched) {
            let m = match got {
                None => continue,
                Some(Ok(m)) => m,
                // The answer was cut off (laptop asleep, connection dropped): leave it queued to fetch again.
                Some(Err(RiotError::BadBody { source, .. })) if !riot_sent_garbage(&source) => {
                    let mut cut_off = self.cut_off.lock().unwrap();
                    let n = cut_off.entry(id.clone()).or_default();
                    *n += 1;
                    if *n < CUT_OFF_LIMIT {
                        println!("[{route}] the answer for {id} was cut off. It will be fetched again.");
                    } else {
                        println!(
                            "[{route}] the answer for {id} was cut off {n} times. Leaving it queued for the next run."
                        );
                    }
                    continue;
                }
                Some(Err(RiotError::BadBody { url, source })) => {
                    println!("[{route}] skipped {id}: Riot sent an unreadable answer ({source}) for {url}");
                    set_status(id, 2)?;
                    continue;
                }
                Some(Err(e)) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            let ranked = m.as_ref().is_some_and(|m| {
                m.info.queue() == Some(self.config.ranked_queue_id) && m.info.participants.len() >= 2
            });
            let Some(m) = m.filter(|_| ranked) else {
                set_status(id, 2)?;
                continue;
            };
            if !self.below_max() {
                continue;
            }
            let ingest = Ingest { platform, rank: *seed_rank, tier: tier_name(*seed_rank), snowball: self.config.snowball };
            let added = {
                let cal = self.calendar.read().unwrap();
                die_at("before_store", id);
                ingest.store(&mut self.db.lock().unwrap(), &m, &cal)?
            };
            die_at("after_store", id);
            if added {
                self.stored.fetch_add(1, Ordering::Relaxed);
            }
            set_status(id, 1)?;
        }
        if let Some(e) = first_err {
            return Err(e.into());
        }
        Ok(false)
    }
}

/// True when Riot's whole answer arrived but was not readable JSON, so fetching it again gives the same.
/// False when the answer was cut off on the way, which is worth another try.
fn riot_sent_garbage(e: &reqwest::Error) -> bool {
    std::error::Error::source(e).is_some_and(|s| s.is::<serde_json::Error>())
}

mod rusqlite_result {
    pub type Result<T> = std::result::Result<T, tft_store::Error>;
}

#[cfg(test)]
mod tests {
    use super::riot_sent_garbage;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves one answer whose body is `body`, announced as `length` bytes long, then hangs up.
    async fn answer_once(body: &'static str, length: usize) -> reqwest::Error {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let head = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {length}\r\n\r\n");
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(body.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
        });
        let res = crate::client::http_client().get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
        res.json::<serde_json::Value>().await.unwrap_err()
    }

    #[tokio::test]
    async fn a_cut_off_answer_is_fetched_again() {
        assert!(!riot_sent_garbage(&answer_once("{\"metadata\":", 500).await));
    }

    #[tokio::test]
    async fn an_unreadable_answer_is_skipped() {
        assert!(riot_sent_garbage(&answer_once("not json", 8).await));
    }
}
