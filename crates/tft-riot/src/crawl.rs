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
//! Rust only (E7): a fixed share of stored matches (`RANK_SAMPLE_RATE`, 1% by default) gets every player's real
//! rank looked up, beside the crawl. See [`Crawler::sample_ranks`].
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
use tft_store::{
    Connection, Ingest, complete_sample, is_sampled, kv_get, kv_set, load_calendar, params, pending_samples,
    queue_sample, save_calendar, save_rank,
};

/// Look at the same player again after this long.
const RECRAWL_MS: i64 = 6 * 3600 * 1000;

/// `kv` key holding the fingerprint (never the key itself) of the last API key Riot accepted. On the next start
/// a rejection of that same key means it has expired, so it is reported at once instead of after the
/// 2-minute activation wait.
const KEY_OK: &str = "riot_key_ok";

/// Settings key: servers whose ladders were not read to the end (the crawler was stopped or killed).
const SEED_PENDING: &str = "seed_pending";

/// A match whose answer is cut off this many times in one run is left queued for the next run.
const CUT_OFF_LIMIT: u32 = 3;

/// Sampled matches looked up together (8 lookups each).
const SAMPLE_BATCH: i64 = 2;

/// A sampled match whose lookups failed is left alone this long before it is tried again.
const SAMPLE_RETRY: Duration = Duration::from_secs(600);

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
    /// A group hit a key error, so every group stops (PRD rule 5).
    key_failed: AtomicBool,
    /// The key's fingerprint is in `kv` already.
    key_saved: AtomicBool,
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
        let riot = RiotClient::new(&config.api_key, &config.api_base, config.rate_margin);
        // An unreadable value just means the key is treated as new.
        let known = kv_get::<String>(&conn, KEY_OK).ok().flatten().is_some_and(|fp| fp == riot.key_fingerprint());
        if known {
            riot.set_key_worked();
        }
        Ok(Arc::new(Crawler {
            riot,
            config,
            opts,
            db: Mutex::new(conn),
            calendar: RwLock::new(calendar),
            stored: AtomicU64::new(0),
            stop,
            key_failed: AtomicBool::new(false),
            key_saved: AtomicBool::new(known),
            cut_off: Mutex::new(HashMap::new()),
        }))
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.key_failed.load(Ordering::Relaxed)
    }

    /// Once Riot has accepted the key, saves its fingerprint so the next start knows it has worked.
    fn remember_key(&self) -> Result<()> {
        if self.riot.key_worked() && !self.key_saved.load(Ordering::Relaxed) {
            kv_set(&self.db.lock().unwrap(), KEY_OK, &self.riot.key_fingerprint())?;
            self.key_saved.store(true, Ordering::Relaxed);
        }
        Ok(())
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
                let seeded = self.seed(platform).await;
                self.remember_key()?;
                seeded?;
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
            async move { me.crawl_and_sample(route, platforms).await }
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
            // Each routing group's share of its long rate-limit window, averaged since its first request.
            let mut groups: Vec<&str> = self.config.platforms.iter().filter_map(|p| platform_route(p)).collect();
            groups.sort();
            groups.dedup();
            let slots: Vec<String> = groups
                .iter()
                .filter_map(|g| self.riot.slot_use(g).map(|u| format!("{g} {:.1}/{}", u.average, u.riot_limit)))
                .collect();
            let slots = if slots.is_empty() { "none yet".to_string() } else { slots.join(", ") };
            println!(
                "stored {stored} this run ({per_hour}/hour) | database: {m} matches, {p} players, {q} queued | requests ok {}, 429s {}, retried {} | slots used per 2 min: {slots}",
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
            let stepped = self.step(route, &platforms_json).await;
            self.remember_key()?;
            match stepped {
                Ok(done) => finished = done,
                Err(e) => match e.downcast_ref::<RiotError>() {
                    // A key problem affects every group, so all of them stop, not only this one.
                    Some(RiotError::Key(_)) => {
                        self.key_failed.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
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

    /// Crawls one routing group and, beside it, looks up real ranks for its sampled matches. With `--once` the
    /// lookups still waiting when the crawl ends are finished before this returns.
    async fn crawl_and_sample(&self, route: &str, platforms: Vec<String>) -> Result<()> {
        let crawled = AtomicBool::new(false);
        let crawl = async {
            let r = self.crawl_route(route, platforms.clone()).await;
            crawled.store(true, Ordering::Relaxed);
            r
        };
        let (c, s) = tokio::join!(crawl, self.sample_ranks(route, &platforms, &crawled));
        c.and(s)
    }

    /// Looks up the real rank of every player in this group's sampled matches (E7), until the crawl has ended
    /// and nothing is left that can be done now.
    ///
    /// - The lookups go to each server's own host (na1, euw1) through that host's limiter. Riot limits each
    ///   host separately, so they do not hold up the group's match downloads, which go to the routing host.
    /// - Every answer is saved at once, and a match is marked done only when all its players are in. A stop or
    ///   kill loses at most the lookups in flight; the next run looks up only the players still missing.
    /// - A lookup Riot does not answer leaves its match waiting: it is tried again after [`SAMPLE_RETRY`], or
    ///   on the next run, never dropped.
    async fn sample_ranks(&self, route: &str, platforms: &[String], crawled: &AtomicBool) -> Result<()> {
        let mut failed: HashMap<String, tokio::time::Instant> = HashMap::new();
        loop {
            if self.stopping() {
                return Ok(());
            }
            failed.retain(|_, at| at.elapsed() < SAMPLE_RETRY);
            let skip: Vec<String> = failed.keys().cloned().collect();
            let batch = pending_samples(&self.db.lock().unwrap(), platforms, &skip, SAMPLE_BATCH)?;
            if batch.is_empty() {
                if crawled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                self.pause(1).await;
                continue;
            }
            let lookups = batch.iter().flat_map(|(id, platform, puuids)| puuids.iter().map(move |p| (id, platform, p)));
            let answers = join_all(lookups.map(|(id, platform, puuid)| async move {
                (id, puuid, self.riot.rank(platform, puuid).await)
            }))
            .await;
            let mut unavailable = None;
            {
                let db = self.db.lock().unwrap();
                for (id, puuid, got) in answers {
                    match got {
                        Ok(entries) => {
                            let e = entries.iter().find(|e| e.queue_type == "RANKED_TFT");
                            let tier = e.map_or(tft_store::UNRANKED, |e| e.tier.as_str());
                            save_rank(&db, id, puuid, tier, e.map_or("", |e| e.rank.as_str()))?;
                        }
                        Err(RiotError::Key(k)) => {
                            self.key_failed.store(true, Ordering::Relaxed);
                            return Err(RiotError::Key(k).into());
                        }
                        Err(e) => {
                            failed.insert(id.clone(), tokio::time::Instant::now());
                            if let RiotError::Unavailable(msg) = e {
                                unavailable.get_or_insert(msg);
                            }
                        }
                    }
                }
                for (id, _, _) in &batch {
                    complete_sample(&db, id)?;
                }
            }
            self.remember_key()?;
            if let Some(msg) = unavailable
                && !crawled.load(Ordering::Relaxed)
            {
                println!("[{route}] {msg} Rank lookups wait a minute; the matches are kept for later.");
                self.pause(60).await;
            }
        }
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
                let mut db = self.db.lock().unwrap();
                let added = ingest.store(&mut db, &m, &cal)?;
                // Queued even when it was stored before (a kill before its status was set), so none is missed.
                if is_sampled(id, self.config.rank_sample_rate) {
                    queue_sample(&db, id, platform)?;
                }
                added
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
    use super::*;
    use crate::client::KeyProblem;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::Instant;

    type Hits = Arc<Mutex<Vec<(Instant, String)>>>;

    /// Like the client tests' `serve()`, but answers by path, because two routing groups send at once.
    /// `answer(path, n)` gets the request path and how many requests that path has seen before, and
    /// returns status, extra header lines (each ending in CRLF) and body.
    async fn serve(answer: fn(&str, usize) -> (u16, &'static str, String)) -> (String, Hits) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits: Hits = Arc::default();
        let seen = Arc::clone(&hits);
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
                let before = {
                    let mut seen = seen.lock().unwrap();
                    let before = seen.iter().filter(|(_, p)| *p == path).count();
                    seen.push((Instant::now(), path.clone()));
                    before
                };
                let (status, headers, body) = answer(&path, before);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                     connection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (format!("http://127.0.0.1:{port}/{{host}}"), hits)
    }

    /// A ranked two-player match with the given id.
    fn match_body(id: &str) -> String {
        serde_json::json!({
            "metadata": { "match_id": id, "participants": ["a", "b"] },
            "info": {
                "game_datetime": 1_000, "game_length": 2100.5, "game_version": "TFT Unreal Version ?.?.?.?",
                "queue_id": 1100, "tft_set_number": 16,
                "participants": [
                    { "puuid": format!("{id}-a"), "placement": 1, "traits": [], "units": [] },
                    { "puuid": format!("{id}-b"), "placement": 2, "traits": [], "units": [] }
                ]
            }
        })
        .to_string()
    }

    const STUCK: &str = "/americas/tft/match/v1/matches/NA1_1";
    const EUROPE_MATCHES: usize = 120;

    /// PRD rule 6, crawler half. americas' only queued match gets six 500s, then answers. europe answers
    /// every match, one per second (its rate-limit header says 1:1), so it is busy for about two minutes.
    fn riot(path: &str, before: usize) -> (u16, &'static str, String) {
        let id = path.rsplit('/').next().unwrap_or_default();
        if path == STUCK {
            return if before < 6 { (500, "", String::new()) } else { (200, "", match_body(id)) };
        }
        if path.contains("/by-puuid/") {
            return (200, "", "[]".into());
        }
        if path.starts_with("/europe/tft/match/v1/matches/") {
            return (200, "x-app-rate-limit: 1:1,1000:120\r\n", match_body(id));
        }
        (404, "", String::new())
    }

    /// Six failed tries on one group pause that group's crawl for 60 s; the other group keeps going, and the
    /// match stays queued until it is fetched after the pause.
    #[tokio::test(start_paused = true)]
    async fn six_server_errors_pause_only_that_group_for_a_minute_and_the_match_stays_queued() {
        let (base, hits) = serve(riot).await;
        let config = Config::from_lookup(|k| match k {
            "RIOT_API_KEY" => Some("test-key".into()),
            "RIOT_API_BASE" => Some(base.clone()),
            "PLATFORMS" => Some("na1,euw1".into()),
            "SNOWBALL" => Some("false".into()), // no new players, so only the queued matches are fetched
            _ => None,
        });
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(tft_store::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO match_queue (match_id, route, platform, seed_rank) VALUES ('NA1_1','americas','na1',9)",
            [],
        )
        .unwrap();
        for i in 0..EUROPE_MATCHES {
            conn.execute(
                "INSERT INTO match_queue (match_id, route, platform, seed_rank) VALUES (?1,'europe','euw1',9)",
                [format!("EUW1_{i}")],
            )
            .unwrap();
        }
        // No request timeout and no idle pool, so the only timers are the client's, limiter's and crawler's.
        crate::client::init_tls();
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        let crawler = Arc::new(Crawler {
            riot: RiotClient::new(&config.api_key, &config.api_base, config.rate_margin).with_http(http),
            opts: CrawlOptions { once: true, ..Default::default() },
            config,
            db: Mutex::new(conn),
            calendar: RwLock::new(Calendar::default()),
            stored: AtomicU64::new(0),
            stop: Arc::default(),
            key_failed: AtomicBool::new(false),
            key_saved: AtomicBool::new(false),
            cut_off: Mutex::new(HashMap::new()),
        });
        let status = |c: &Crawler| -> i64 {
            let db = c.db.lock().unwrap();
            db.query_row("SELECT status FROM match_queue WHERE match_id = 'NA1_1'", [], |r| r.get(0)).unwrap()
        };

        let me = Arc::clone(&crawler);
        let crawl = tokio::spawn(async move {
            let (a, e) = tokio::join!(
                me.crawl_route("americas", vec!["na1".into()]),
                me.crawl_route("europe", vec!["euw1".into()])
            );
            a.and(e)
        });

        // Wait for the sixth failed try, then look at the queue during the pause.
        let sixth = loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stuck: Vec<Instant> =
                hits.lock().unwrap().iter().filter(|(_, p)| p == STUCK).map(|(t, _)| *t).collect();
            if stuck.len() >= 6 {
                break stuck[5];
            }
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(status(&crawler), 0, "the match must stay queued after Unavailable");

        crawl.await.unwrap().unwrap();
        let hits = hits.lock().unwrap().clone();
        let americas: Vec<Instant> =
            hits.iter().filter(|(_, p)| p.starts_with("/americas/")).map(|(t, _)| *t).collect();
        assert_eq!(americas.len(), 7, "six failed tries, then one after the pause");
        let waited = americas[6] - sixth;
        assert!(waited >= Duration::from_secs(60), "americas went again {waited:?} after giving up, expected 60 s");
        let europe_during =
            hits.iter().filter(|(t, p)| p.starts_with("/europe/") && *t > sixth && *t < americas[6]).count();
        assert!(europe_during >= 30, "europe sent only {europe_during} requests during americas' pause");

        assert_eq!(status(&crawler), 1, "the match is fetched and stored after the pause");
        let stored: i64 = crawler
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM matches WHERE match_id = 'NA1_1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, 1);
        let europe_left: i64 = crawler
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM match_queue WHERE route = 'europe' AND status = 0", [], |r| r.get(0))
            .unwrap();
        assert_eq!(europe_left, 0, "europe finished its queue");
    }

    fn test_config(base: &str, key: &str) -> Config {
        Config::from_lookup(|k| match k {
            "RIOT_API_KEY" => Some(key.into()),
            "RIOT_API_BASE" => Some(base.into()),
            "PLATFORMS" => Some("na1,euw1".into()),
            "SNOWBALL" => Some("false".into()),
            _ => None,
        })
    }

    fn queue(conn: &Connection, id: &str, route: &str, platform: &str) {
        conn.execute(
            "INSERT INTO match_queue (match_id, route, platform, seed_rank) VALUES (?1, ?2, ?3, 9)",
            params![id, route, platform],
        )
        .unwrap();
    }

    /// A crawler on `conn` built by `Crawler::new` (so it reads the remembered key), with the test HTTP client.
    fn crawler_on(conn: Connection, base: &str, key: &str) -> Arc<Crawler> {
        let opts = CrawlOptions { once: true, ..Default::default() };
        let mut crawler = Crawler::new(test_config(base, key), opts, conn, Arc::default()).unwrap();
        crate::client::init_tls();
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        let me = Arc::get_mut(&mut crawler).unwrap();
        let worked = me.riot.key_worked();
        let riot = std::mem::replace(&mut me.riot, RiotClient::new(key, base, 1.0));
        me.riot = riot.with_http(http);
        assert_eq!(me.riot.key_worked(), worked);
        crawler
    }

    fn remembered(c: &Crawler) -> Option<String> {
        kv_get::<String>(&c.db.lock().unwrap(), KEY_OK).unwrap()
    }

    fn take_db(c: Arc<Crawler>) -> Connection {
        Arc::try_unwrap(c).ok().expect("one owner").db.into_inner().unwrap()
    }

    fn accept_all(path: &str, _: usize) -> (u16, &'static str, String) {
        (200, "", match_body(path.rsplit('/').next().unwrap_or_default()))
    }

    fn reject_all(_: &str, _: usize) -> (u16, &'static str, String) {
        (403, "", "{}".into())
    }

    const KEY: &str = "RGAPI-test-key-one";

    /// A success is remembered by fingerprint only, and survives a restart: a new crawler with the same key
    /// treats a rejection as expired (one request, at once). A different key is new and gets the wait.
    #[tokio::test(start_paused = true)]
    async fn a_key_that_worked_is_remembered_across_restarts_by_fingerprint_only() {
        let (base, _) = serve(accept_all).await;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(tft_store::schema::SCHEMA).unwrap();
        queue(&conn, "NA1_1", "americas", "na1");
        let first = crawler_on(conn, &base, KEY);
        assert!(!first.riot.key_worked(), "a fresh database knows no key");
        first.crawl_route("americas", vec!["na1".into()]).await.unwrap();
        let fp = remembered(&first).expect("a success saves the key's fingerprint");
        assert_eq!(fp, first.riot.key_fingerprint());
        let raw: String =
            first.db.lock().unwrap().query_row("SELECT value FROM kv WHERE key = ?1", [KEY_OK], |r| r.get(0)).unwrap();
        assert!(!raw.contains(KEY) && !raw.contains("test-key"), "the key itself must never be stored: {raw}");

        // Restart with the same key, and Riot now rejects it.
        let (base, hits) = serve(reject_all).await;
        let mut conn = take_db(first);
        queue(&conn, "NA1_2", "americas", "na1");
        let second = crawler_on(conn, &base, KEY);
        assert!(second.riot.key_worked(), "the same key is known to have worked");
        let started = Instant::now();
        let err = second.crawl_route("americas", vec!["na1".into()]).await.unwrap_err();
        assert!(matches!(err.downcast_ref::<RiotError>(), Some(RiotError::Key(KeyProblem::Expired))), "got {err:?}");
        assert_eq!(hits.lock().unwrap().len(), 1, "an expired key is reported after one request");
        assert!(Instant::now() - started < Duration::from_secs(1));

        // A different key is new: it gets the activation wait, then is reported as not active.
        conn = take_db(second);
        let (base, hits) = serve(reject_all).await;
        let third = crawler_on(conn, &base, "RGAPI-test-key-two");
        assert!(!third.riot.key_worked(), "a different key has not worked yet");
        let started = Instant::now();
        let err = third.crawl_route("americas", vec!["na1".into()]).await.unwrap_err();
        assert!(matches!(err.downcast_ref::<RiotError>(), Some(RiotError::Key(KeyProblem::NotActive))), "got {err:?}");
        assert!(Instant::now() - started >= crate::client::ACTIVATION_WAIT);
        assert!(hits.lock().unwrap().len() > 1);
        assert_eq!(remembered(&third), Some(fp), "a key that never worked does not replace the remembered one");
    }

    fn americas_rejected(path: &str, _: usize) -> (u16, &'static str, String) {
        let id = path.rsplit('/').next().unwrap_or_default();
        if path.starts_with("/americas/") {
            return (403, "", "{}".into());
        }
        (200, "x-app-rate-limit: 1:1,1000:120\r\n", match_body(id))
    }

    /// PRD rule 5: a key error in one group stops every group, not only the one that got it.
    #[tokio::test(start_paused = true)]
    async fn a_key_error_in_one_group_stops_all_groups() {
        let (base, hits) = serve(americas_rejected).await;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(tft_store::schema::SCHEMA).unwrap();
        queue(&conn, "NA1_1", "americas", "na1");
        for i in 0..EUROPE_MATCHES {
            queue(&conn, &format!("EUW1_{i}"), "europe", "euw1");
        }
        let crawler = crawler_on(conn, &base, KEY);
        crawler.riot.set_key_worked(); // so the rejection is reported at once

        let (a, e) = tokio::join!(
            crawler.crawl_route("americas", vec!["na1".into()]),
            crawler.crawl_route("europe", vec!["euw1".into()])
        );
        let err = a.unwrap_err();
        assert!(matches!(err.downcast_ref::<RiotError>(), Some(RiotError::Key(KeyProblem::Expired))), "got {err:?}");
        e.unwrap();
        let hits = hits.lock().unwrap().clone();
        let rejected = hits.iter().find(|(_, p)| p.starts_with("/americas/")).unwrap().0;
        let after = hits.iter().filter(|(t, p)| p.starts_with("/europe/") && *t > rejected).count();
        // Like Ctrl+C, a stop lets the batch already in flight (up to 20 matches) finish, and nothing after it.
        assert!(after <= 20, "europe sent {after} requests after the key error; only its batch in flight may finish");
        let europe_left: i64 = crawler
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM match_queue WHERE route = 'europe' AND status = 0", [], |r| r.get(0))
            .unwrap();
        assert!(europe_left >= 80, "europe kept crawling: only {europe_left} matches left in its queue");
    }

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

    // ---- Rank samples (E7) ----------------------------------------------------------------------

    /// A ranked eight-player match; player `i` is `{id}-{i}`.
    fn lobby_body(id: &str) -> String {
        let players: Vec<serde_json::Value> = (0..8)
            .map(|i| serde_json::json!({ "puuid": format!("{id}-{i}"), "placement": i + 1, "traits": [], "units": [] }))
            .collect();
        serde_json::json!({
            "metadata": { "match_id": id, "participants": [] },
            "info": { "game_datetime": 1_000, "game_version": "TFT Unreal Version ?.?.?.?", "queue_id": 1100,
                      "tft_set_number": 16, "participants": players }
        })
        .to_string()
    }

    /// The real tier the fake Riot gives player `i` of a lobby. Player 3 has no ranked entry.
    const REAL: [&str; 8] = ["GOLD", "PLATINUM", "EMERALD", "", "DIAMOND", "MASTER", "GRANDMASTER", "CHALLENGER"];

    fn rank_body(puuid: &str) -> String {
        let i: usize = puuid.rsplit('-').next().unwrap().parse().unwrap();
        if REAL[i].is_empty() {
            return "[]".into();
        }
        serde_json::json!([
            { "queueType": "RANKED_TFT_TURBO", "tier": "ORANGE", "rank": "I", "leaguePoints": 0, "wins": 0, "losses": 0 },
            { "queueType": "RANKED_TFT", "tier": REAL[i], "rank": "II", "leaguePoints": 10, "wins": 1, "losses": 2 }
        ])
        .to_string()
    }

    fn lobbies(path: &str, _: usize) -> (u16, &'static str, String) {
        let last = path.rsplit('/').next().unwrap_or_default();
        if path.contains("/tft/league/v1/by-puuid/") {
            return (200, "", rank_body(last));
        }
        (200, "", lobby_body(last))
    }

    /// Player 7's lookups fail six times in a row (Riot unavailable), then answer.
    fn player_seven_flaky(path: &str, before: usize) -> (u16, &'static str, String) {
        if path.ends_with("-7") && before < 6 {
            return (500, "", String::new());
        }
        lobbies(path, before)
    }

    /// One answer per second on na1, so lookups are in flight long enough to be cut short.
    fn slow_ranks(path: &str, before: usize) -> (u16, &'static str, String) {
        let (status, _, body) = lobbies(path, before);
        (status, "x-app-rate-limit: 1:1,1000:1000\r\n", body)
    }

    fn sample_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(tft_store::schema::SCHEMA).unwrap();
        tft_store::apply_migrations(&conn).unwrap();
        conn
    }

    fn sampler_on(conn: Connection, base: &str, rate: f64) -> Arc<Crawler> {
        let mut config = test_config(base, KEY);
        config.rank_sample_rate = rate;
        let opts = CrawlOptions { once: true, ..Default::default() };
        let mut crawler = Crawler::new(config, opts, conn, Arc::default()).unwrap();
        crate::client::init_tls();
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        let me = Arc::get_mut(&mut crawler).unwrap();
        let riot = std::mem::replace(&mut me.riot, RiotClient::new(KEY, base, 1.0));
        me.riot = riot.with_http(http);
        me.riot.set_key_worked();
        crawler
    }

    fn rows(c: &Crawler, sql: &str) -> Vec<(String, String)> {
        let db = c.db.lock().unwrap();
        let mut q = db.prepare(sql).unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    }

    fn count(c: &Crawler, sql: &str) -> i64 {
        c.db.lock().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Stores lobbies `NA1_0..n` directly and queues the sampled ones, as `step` does.
    fn stored_lobbies(n: usize, rate: f64) -> (Connection, Vec<String>) {
        let mut conn = sample_db();
        let mut sampled = Vec::new();
        for i in 0..n {
            let id = format!("NA1_{i}");
            let m: tft_core::riot::RiotMatch = serde_json::from_str(&lobby_body(&id)).unwrap();
            let ingest = Ingest { platform: "na1", rank: 7, tier: "MASTER", snowball: false };
            ingest.store(&mut conn, &m, &Calendar::default()).unwrap();
            if is_sampled(&id, rate) {
                queue_sample(&conn, &id, "na1").unwrap();
                sampled.push(id);
            }
        }
        (conn, sampled)
    }

    /// E7: the crawl queues exactly the matches the hash picks, and each gets all 8 real ranks stored (the
    /// ranked TFT entry, not another queue's; a player with no entry is stored as unranked).
    #[tokio::test(start_paused = true)]
    async fn sampled_matches_get_all_eight_ranks_looked_up() {
        let (base, hits) = serve(lobbies).await;
        let conn = sample_db();
        let ids: Vec<String> = (0..60).map(|i| format!("NA1_{i}")).collect();
        for id in &ids {
            queue(&conn, id, "americas", "na1");
        }
        let rate = 0.25;
        let expected: Vec<String> = ids.iter().filter(|id| is_sampled(id, rate)).cloned().collect();
        assert!(expected.len() > 5 && expected.len() < 30, "{} of 60 picked", expected.len());

        let crawler = sampler_on(conn, &base, rate);
        crawler.crawl_and_sample("americas", vec!["na1".into()]).await.unwrap();

        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM matches"), 60, "every match is stored");
        let mut queued: Vec<String> =
            rows(&crawler, "SELECT match_id, platform FROM rank_sample_queue WHERE done = 1").into_iter().map(|r| r.0).collect();
        queued.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(queued, want, "the sampled matches are the hash's picks, and all are done");
        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM rank_sample_queue WHERE done = 0"), 0);
        for id in &expected {
            let got = rows(&crawler, &format!("SELECT puuid, tier FROM rank_samples WHERE match_id = '{id}' ORDER BY puuid"));
            let want: Vec<(String, String)> = (0..8)
                .map(|i| (format!("{id}-{i}"), if REAL[i].is_empty() { "UNRANKED" } else { REAL[i] }.to_string()))
                .collect();
            assert_eq!(got, want, "{id}");
        }
        let divisions = rows(&crawler, "SELECT DISTINCT division, '' FROM rank_samples ORDER BY 1");
        assert_eq!(divisions, [("".to_string(), "".to_string()), ("II".to_string(), "".to_string())]);
        let lookups = hits.lock().unwrap().iter().filter(|(_, p)| p.starts_with("/na1/tft/league/v1/by-puuid/")).count();
        assert_eq!(lookups, expected.len() * 8, "one lookup per player, on the server's own host");

        let a = tft_store::rank_sample_accuracy(&crawler.db.lock().unwrap()).unwrap();
        // Stored as Master; 3 of 8 reach Master, so the lobby is really Diamond+ by the upper-median rule.
        assert_eq!((a.sampled_matches, a.players, a.share_in_right_bucket), (expected.len() as i64, expected.len() as i64 * 8, Some(0.0)));
    }

    /// With the rate at 0 nothing is sampled and no lookups are sent, so the crawl is the same as before E7.
    #[tokio::test(start_paused = true)]
    async fn rate_zero_samples_nothing() {
        let (base, hits) = serve(lobbies).await;
        let conn = sample_db();
        for i in 0..60 {
            queue(&conn, &format!("NA1_{i}"), "americas", "na1");
        }
        let crawler = sampler_on(conn, &base, 0.0);
        crawler.crawl_and_sample("americas", vec!["na1".into()]).await.unwrap();
        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM matches"), 60);
        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM rank_sample_queue"), 0);
        assert!(!hits.lock().unwrap().iter().any(|(_, p)| p.contains("/by-puuid/")));
    }

    /// A lookup Riot does not answer leaves its match waiting, not dropped and not marked done; after a restart
    /// only the missing player is looked up and the match is finished.
    #[tokio::test(start_paused = true)]
    async fn a_failed_lookup_is_kept_and_finished_after_a_restart() {
        let (base, hits) = serve(player_seven_flaky).await;
        let (conn, sampled) = stored_lobbies(1, 1.0);
        let crawler = sampler_on(conn, &base, 1.0);
        crawler.sample_ranks("americas", &["na1".into()], &AtomicBool::new(true)).await.unwrap();
        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM rank_samples"), 7, "seven answers saved");
        assert_eq!(count(&crawler, "SELECT done FROM rank_sample_queue"), 0, "the match is still waiting");
        assert_eq!(tft_store::rank_sample_accuracy(&crawler.db.lock().unwrap()).unwrap().sampled_matches, 0);

        let restarted = sampler_on(take_db(crawler), &base, 1.0);
        restarted.sample_ranks("americas", &["na1".into()], &AtomicBool::new(true)).await.unwrap();
        assert_eq!(count(&restarted, "SELECT COUNT(*) FROM rank_samples"), 8);
        assert_eq!(count(&restarted, "SELECT done FROM rank_sample_queue"), 1);
        let hits = hits.lock().unwrap().clone();
        for i in 0..7 {
            let path = format!("/na1/tft/league/v1/by-puuid/{}-{i}", sampled[0]);
            assert_eq!(hits.iter().filter(|(_, p)| *p == path).count(), 1, "{path} looked up again");
        }
    }

    /// While the crawl is still running, a failed match is tried again after a pause in the same run.
    #[tokio::test(start_paused = true)]
    async fn a_failed_lookup_is_tried_again_while_the_crawl_runs() {
        let (base, _) = serve(player_seven_flaky).await;
        let (conn, _) = stored_lobbies(1, 1.0);
        let crawler = sampler_on(conn, &base, 1.0);
        let crawled = AtomicBool::new(false);
        let platforms = ["na1".to_string()];
        let sampler = crawler.sample_ranks("americas", &platforms, &crawled);
        let finish = async {
            while count(&crawler, "SELECT COUNT(*) FROM rank_sample_queue WHERE done = 1") == 0 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            crawled.store(true, Ordering::Relaxed);
        };
        let (r, ()) = tokio::time::timeout(SAMPLE_RETRY * 2, async { tokio::join!(sampler, finish) }).await.unwrap();
        r.unwrap();
        assert_eq!(count(&crawler, "SELECT COUNT(*) FROM rank_samples"), 8);
    }

    /// Killed mid-lookups (the task is dropped, so nothing after that point runs): every answer already in is
    /// kept, and the restart looks up only what is missing and finishes every sampled match.
    #[tokio::test(start_paused = true)]
    async fn lookups_cut_short_by_a_kill_resume_without_losing_or_repeating_work() {
        let (base, hits) = serve(slow_ranks).await;
        let (conn, sampled) = stored_lobbies(40, 0.2);
        assert!(sampled.len() >= 4, "{} sampled", sampled.len());
        let crawler = sampler_on(conn, &base, 0.2);
        let me = Arc::clone(&crawler);
        let task = tokio::spawn(async move { me.sample_ranks("americas", &["na1".into()], &AtomicBool::new(true)).await });
        while count(&crawler, "SELECT COUNT(*) FROM rank_samples") < 20 {
            assert!(!task.is_finished(), "the lookups ended before 20 answers were in");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        task.abort();
        let _ = task.await;
        let saved = count(&crawler, "SELECT COUNT(*) FROM rank_samples");
        let total = sampled.len() as i64 * 8;
        assert!(saved < total, "the kill landed before the end ({saved} of {total})");
        let asked_before = hits.lock().unwrap().len();

        let restarted = sampler_on(take_db(crawler), &base, 0.2);
        restarted.sample_ranks("americas", &["na1".into()], &AtomicBool::new(true)).await.unwrap();
        assert_eq!(count(&restarted, "SELECT COUNT(*) FROM rank_samples"), total);
        assert_eq!(count(&restarted, "SELECT COUNT(*) FROM rank_sample_queue WHERE done = 1"), sampled.len() as i64);
        let asked_after = hits.lock().unwrap().len() - asked_before;
        // Only the missing players are asked for (in-flight lookups at the kill were never answered).
        assert_eq!(asked_after as i64, total - saved, "the restart asks only for the players still missing");
    }
}
