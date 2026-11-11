//! The engine's one supervisor task (PRD 1, Scheduler). It owns:
//!
//! - the crawl loop, always;
//! - the stats tick, every 10 minutes, only when new matches arrived (or matches were given a new patch);
//! - the calendar and set data refresh, every 3 hours, and once at start;
//! - prune and compact, daily, only when the crawler is idle (a placeholder until prune is built).
//!
//! The timing lives in [`supervise`], which drives any [`Jobs`]; [`run_supervisor`] runs it with the real ones.

use crate::cdragon::{Refreshed, Source, refresh};
use crate::crawl::Crawler;
use anyhow::{Context, Result};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tft_store::{Connection, ERROR, WARN};
use tokio::time::{Instant, MissedTickBehavior, interval, interval_at};

/// How often each job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub stats: Duration,
    pub refresh: Duration,
    pub prune: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            stats: Duration::from_secs(10 * 60),
            refresh: Duration::from_secs(3 * 3600),
            prune: Duration::from_secs(24 * 3600),
        }
    }
}

/// The work the supervisor schedules. The crawl loop is passed to [`supervise`] separately.
pub trait Jobs: Send {
    /// Whether matches were stored since the last stats build.
    fn new_matches(&mut self) -> Result<bool>;
    /// Rebuilds the stat tables.
    fn stats(&mut self) -> impl Future<Output = Result<()>> + Send;
    /// Downloads the patch calendar and set data. See [`refresh`].
    fn refresh(&mut self) -> impl Future<Output = Result<Refreshed>> + Send;
    /// Whether nothing has been stored since the last check, so heavy database work will not hold up the crawler.
    fn idle(&mut self) -> Result<bool>;
    /// Prunes and compacts the database.
    fn prune_when_idle(&mut self) -> Result<()>;
    /// Notes a failed job in the engine log (E10). `level` is [`tft_store::WARN`] or [`tft_store::ERROR`].
    fn log(&mut self, _level: &str, _message: &str) {}
}

/// Runs `crawl` and schedules `jobs` around it until `crawl` ends, then returns what `crawl` returned.
/// A job that fails is reported and tried again at its next turn; it never stops the crawl.
pub async fn supervise<J, C>(crawl: C, jobs: &mut J, t: Timings) -> Result<()>
where
    J: Jobs,
    C: Future<Output = Result<()>> + Send + 'static,
{
    let mut crawl = tokio::spawn(crawl);
    let mut stats = interval(t.stats);
    let mut refresh = interval(t.refresh);
    let mut prune = interval_at(Instant::now() + t.prune, t.prune);
    for i in [&mut stats, &mut refresh, &mut prune] {
        i.set_missed_tick_behavior(MissedTickBehavior::Delay);
    }
    // Matches given a new patch need a stats build even though no new rows arrived.
    let mut relabelled = false;
    let mut prune_due = false;

    loop {
        tokio::select! {
            biased;
            done = &mut crawl => return done.context("the crawl task failed")?,
            _ = refresh.tick() => match jobs.refresh().await {
                Ok(r) => {
                    if let Some(p) = &r.new_patch {
                        println!("New patch {p}: patch calendar updated.");
                    }
                    if let Some(s) = &r.static_set {
                        println!("Set data updated: set {} ({}), {} units, {} traits, {} items.", s.set_number, s.set_name, s.units, s.traits, s.items);
                    }
                    if let Some(e) = &r.static_error {
                        let hours = t.refresh.as_secs() / 3600;
                        let msg = format!("Could not download the set data ({e}). Trying again in {hours} h.");
                        eprintln!("{msg}");
                        jobs.log(WARN, &msg);
                    }
                    relabelled |= r.relabelled > 0;
                }
                Err(e) => {
                    let msg = format!("Could not download the patch calendar ({e:#}). Keeping the saved one.");
                    eprintln!("{msg}");
                    jobs.log(WARN, &msg);
                }
            },
            // Due once a day; runs at the first stats tick from then on when the crawler is idle.
            _ = prune.tick() => prune_due = true,
            _ = stats.tick() => {
                match jobs.new_matches() {
                    Ok(fresh) if fresh || relabelled => match jobs.stats().await {
                        Ok(()) => relabelled = false,
                        Err(e) => {
                            let msg = format!("Building the stats failed ({e:#}). Trying again in 10 minutes.");
                            eprintln!("{msg}");
                            jobs.log(ERROR, &msg);
                        }
                    },
                    Ok(_) => {}
                    Err(e) => {
                        let msg = format!("Could not check for new matches ({e:#}).");
                        eprintln!("{msg}");
                        jobs.log(ERROR, &msg);
                    }
                }
                if prune_due && jobs.idle().unwrap_or(false) {
                    if let Err(e) = jobs.prune_when_idle() {
                        let msg = format!("Pruning the database failed ({e:#}). Trying again tomorrow.");
                        eprintln!("{msg}");
                        jobs.log(ERROR, &msg);
                    }
                    prune_due = false;
                }
            },
        }
    }
}

/// Settings for [`run_supervisor`].
#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    /// The database file. The supervisor opens its own connections to it.
    pub db_path: PathBuf,
    /// How many of the most recent patches the stats tick rebuilds.
    pub patch_count: i64,
    pub source: Source,
    pub timings: Timings,
}

/// Runs the crawler with the stats, refresh and prune jobs around it, until the crawler stops.
pub async fn run_supervisor(crawler: Arc<Crawler>, opts: SupervisorOptions) -> Result<()> {
    let mut jobs = EngineJobs::new(&opts, Some(Arc::clone(&crawler)))?;
    let crawl = async move { crawler.run().await };
    supervise(crawl, &mut jobs, opts.timings).await
}

/// Raw rows are kept for this many patches; older patches keep only their stat tables (E8).
pub const PATCHES_KEPT: usize = 3;

/// The one place the stats step is called. Incremental (E5): only scopes with new matches are rebuilt, and the
/// result equals a full rebuild.
pub fn build_stats_step(conn: &mut Connection, patch_count: i64) -> Result<()> {
    tft_stats::build_stats_incremental_with(conn, patch_count, &mut |line| println!("{line}"))?;
    Ok(())
}

/// Daily, when the crawler is idle: drop raw rows of patches older than the newest [`PATCHES_KEPT`], then compact.
pub fn prune_when_idle(conn: &mut Connection) -> Result<()> {
    let pruned = tft_store::prune_old_patches(conn, PATCHES_KEPT)?;
    if !pruned.patches.is_empty() {
        let c = tft_store::compact(conn)?;
        println!(
            "Pruned raw matches of patch {} ({} matches); database {} MB -> {} MB",
            pruned.patches.join(", "),
            pruned.matches,
            c.before / 1_000_000,
            c.after / 1_000_000
        );
    }
    Ok(())
}

fn newest_match(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(rowid), 0) FROM matches", [], |r| r.get(0))?)
}

/// The real jobs, against the database file.
pub struct EngineJobs {
    conn: Connection,
    db_path: PathBuf,
    patch_count: i64,
    source: Source,
    crawler: Option<Arc<Crawler>>,
    /// The newest match row the last stats build included.
    built: i64,
    /// The newest match row at the last idle check.
    seen: i64,
}

impl EngineJobs {
    /// `crawler`, when given, is told about a new patch calendar at once so new matches are dated right.
    pub fn new(opts: &SupervisorOptions, crawler: Option<Arc<Crawler>>) -> Result<Self> {
        let conn = tft_store::open(&opts.db_path)?;
        Ok(EngineJobs {
            conn,
            db_path: opts.db_path.clone(),
            patch_count: opts.patch_count,
            source: opts.source.clone(),
            crawler,
            built: 0,
            seen: 0,
        })
    }
}

impl Jobs for EngineJobs {
    fn new_matches(&mut self) -> Result<bool> {
        Ok(newest_match(&self.conn)? > self.built)
    }

    async fn stats(&mut self) -> Result<()> {
        let upto = newest_match(&self.conn)?;
        let (path, n) = (self.db_path.clone(), self.patch_count);
        // A full build takes a while: off the async threads, on its own connection.
        tokio::task::spawn_blocking(move || build_stats_step(&mut tft_store::open(&path)?, n)).await??;
        self.built = upto;
        Ok(())
    }

    async fn refresh(&mut self) -> Result<Refreshed> {
        let r = refresh(&mut self.conn, &self.source).await?;
        if r.new_patch.is_some()
            && let Some(c) = &self.crawler
        {
            c.refresh_calendar().await;
            // Matches the crawler stored with the old calendar while it was being downloaded.
            let cal = tft_store::load_calendar(&self.conn)?;
            let late = tft_store::relabel_unknown(&mut self.conn, &cal)?;
            return Ok(Refreshed { relabelled: r.relabelled + late, ..r });
        }
        Ok(r)
    }

    fn idle(&mut self) -> Result<bool> {
        let now = newest_match(&self.conn)?;
        let idle = now == self.seen;
        self.seen = now;
        Ok(idle)
    }

    fn prune_when_idle(&mut self) -> Result<()> {
        prune_when_idle(&mut self.conn)
    }

    fn log(&mut self, level: &str, message: &str) {
        // Never fails a job: a log row that cannot be written is only lost.
        let _ = tft_store::log_event(&self.conn, "", level, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts what ran and when (minutes since start), and is told what the database would say.
    #[derive(Default)]
    struct Fake {
        log: Arc<Mutex<Vec<(u64, &'static str)>>>,
        start: Option<Instant>,
        /// Answers to `new_matches`, in turn; `false` once used up.
        fresh: Vec<bool>,
        /// `relabelled` reported by each refresh, in turn; 0 once used up.
        relabel: Vec<usize>,
        idle: Vec<bool>,
        checks: Arc<AtomicUsize>,
    }

    impl Fake {
        fn note(&self, what: &'static str) {
            let mins = (Instant::now() - self.start.unwrap()).as_secs() / 60;
            self.log.lock().unwrap().push((mins, what));
        }
    }

    impl Jobs for Fake {
        fn new_matches(&mut self) -> Result<bool> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            Ok(if self.fresh.is_empty() { false } else { self.fresh.remove(0) })
        }
        async fn stats(&mut self) -> Result<()> {
            self.note("stats");
            Ok(())
        }
        async fn refresh(&mut self) -> Result<Refreshed> {
            self.note("refresh");
            let relabelled = if self.relabel.is_empty() { 0 } else { self.relabel.remove(0) };
            Ok(Refreshed { relabelled, ..Default::default() })
        }
        fn idle(&mut self) -> Result<bool> {
            Ok(if self.idle.is_empty() { true } else { self.idle.remove(0) })
        }
        fn prune_when_idle(&mut self) -> Result<()> {
            self.note("prune");
            Ok(())
        }
    }

    fn times(log: &Arc<Mutex<Vec<(u64, &'static str)>>>, what: &str) -> Vec<u64> {
        log.lock().unwrap().iter().filter(|(_, w)| *w == what).map(|(t, _)| *t).collect()
    }

    /// Runs the supervisor for `hours` of paused time with a crawl that ends then.
    async fn run_for(hours: u64, fake: &mut Fake) {
        fake.start = Some(Instant::now());
        let crawl = async move {
            tokio::time::sleep(Duration::from_secs(hours * 3600) + Duration::from_secs(1)).await;
            Ok(())
        };
        supervise(crawl, fake, Timings::default()).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stats_run_only_on_ticks_with_new_matches() {
        // Ticks at 0, 10, 20, 30, 40, 50, 60 minutes; new matches at 10 and 40 only.
        let mut fake = Fake { fresh: vec![false, true, false, false, true, false, false], ..Default::default() };
        let log = Arc::clone(&fake.log);
        run_for(1, &mut fake).await;
        assert_eq!(times(&log, "stats"), [10, 40]);
        assert_eq!(fake.checks.load(Ordering::SeqCst), 7, "one check every 10 minutes");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_runs_at_start_and_every_3_hours() {
        let mut fake = Fake::default();
        let log = Arc::clone(&fake.log);
        run_for(9, &mut fake).await;
        assert_eq!(times(&log, "refresh"), [0, 180, 360, 540]);
        assert!(times(&log, "stats").is_empty(), "no new matches, no stats");
    }

    #[tokio::test(start_paused = true)]
    async fn matches_moved_to_a_new_patch_get_a_stats_build() {
        // The refresh at 3 h relabels matches; the next tick builds even though no rows were added.
        let mut fake = Fake { relabel: vec![0, 5], ..Default::default() };
        let log = Arc::clone(&fake.log);
        run_for(4, &mut fake).await;
        assert_eq!(times(&log, "stats"), [180]);
    }

    #[tokio::test(start_paused = true)]
    async fn prune_runs_daily_and_waits_for_idle() {
        // Due at 24 h; busy at 24 h 00 and 24 h 10, idle at 24 h 20. Due again at 48 h, idle at once.
        let mut fake = Fake { idle: vec![false, false, true], ..Default::default() };
        let log = Arc::clone(&fake.log);
        run_for(49, &mut fake).await;
        assert_eq!(times(&log, "prune"), [24 * 60 + 20, 48 * 60]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_supervisor_ends_with_the_crawl_and_returns_its_error() {
        let mut fake = Fake { start: Some(Instant::now()), ..Default::default() };
        let crawl = async { anyhow::bail!("key expired") };
        let err = supervise(crawl, &mut fake, Timings::default()).await.unwrap_err();
        assert_eq!(err.to_string(), "key expired");
    }

    /// Jobs whose stats build always fails, and which note what they log.
    #[derive(Default)]
    struct Failing {
        logged: Vec<(String, String)>,
    }

    impl Jobs for Failing {
        fn new_matches(&mut self) -> Result<bool> {
            Ok(true)
        }
        async fn stats(&mut self) -> Result<()> {
            anyhow::bail!("disk full")
        }
        async fn refresh(&mut self) -> Result<Refreshed> {
            anyhow::bail!("no network")
        }
        fn idle(&mut self) -> Result<bool> {
            Ok(true)
        }
        fn prune_when_idle(&mut self) -> Result<()> {
            anyhow::bail!("locked")
        }
        fn log(&mut self, level: &str, message: &str) {
            self.logged.push((level.to_string(), message.to_string()));
        }
    }

    /// E10: every failed job is noted in the log, and the crawl goes on.
    #[tokio::test(start_paused = true)]
    async fn failed_jobs_are_logged() {
        let mut jobs = Failing::default();
        let crawl = async {
            tokio::time::sleep(Duration::from_secs(24 * 3600 + 60)).await;
            Ok(())
        };
        supervise(crawl, &mut jobs, Timings::default()).await.unwrap();
        let has = |level: &str, text: &str| jobs.logged.iter().any(|(l, m)| l == level && m.contains(text));
        assert!(has(WARN, "Could not download the patch calendar (no network)"), "{:?}", jobs.logged);
        assert!(has(ERROR, "Building the stats failed (disk full)"));
        assert!(has(ERROR, "Pruning the database failed (locked)"));
        let stats = jobs.logged.iter().filter(|(_, m)| m.starts_with("Building the stats")).count();
        assert_eq!(stats, 24 * 6 + 1, "one per 10-minute tick");
    }

    // ---- The real jobs, against a database file ----

    fn temp_db(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("hertz-supervisor-{}-{name}.db", std::process::id()));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
        p
    }

    fn add_match(conn: &Connection, id: &str, patch: &str) {
        conn.execute(
            "INSERT INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank) VALUES (?1,'na1',?2,16,1000,2000,1)",
            [id, patch],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn engine_jobs_see_new_matches_only_once_built() {
        let path = temp_db("fresh");
        let opts = SupervisorOptions {
            db_path: path.clone(),
            patch_count: 2,
            source: Source { http: reqwest::Client::new(), base: "http://127.0.0.1:9".into() },
            timings: Timings::default(),
        };
        let mut jobs = EngineJobs::new(&opts, None).unwrap();
        assert!(!jobs.new_matches().unwrap(), "empty database");
        let conn = tft_store::open(&path).unwrap();
        add_match(&conn, "NA1_1", "16.21");
        assert!(jobs.new_matches().unwrap());
        jobs.stats().await.unwrap();
        assert!(!jobs.new_matches().unwrap(), "built, nothing newer");
        let built: i64 = conn.query_row("SELECT COUNT(*) FROM stat_meta WHERE patch = '16.21'", [], |r| r.get(0)).unwrap();
        assert!(built > 0, "the stats step ran");
        add_match(&conn, "NA1_2", "16.21");
        assert!(jobs.new_matches().unwrap());

        // Idle: nothing stored between two checks.
        assert!(!jobs.idle().unwrap(), "first check sees the rows as new");
        assert!(jobs.idle().unwrap());
        add_match(&conn, "NA1_3", "16.21");
        assert!(!jobs.idle().unwrap());
    }

    /// The real jobs write the engine log; a calendar download that fails shows as the last error.
    #[tokio::test]
    async fn engine_jobs_log_a_failed_refresh() {
        crate::client::init_tls();
        let path = temp_db("log");
        let opts = SupervisorOptions {
            db_path: path.clone(),
            patch_count: 2,
            source: Source { http: reqwest::Client::new(), base: "http://127.0.0.1:9".into() },
            timings: Timings::default(),
        };
        let mut jobs = EngineJobs::new(&opts, None).unwrap();
        let crawl = async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        };
        supervise(crawl, &mut jobs, Timings::default()).await.unwrap();
        let last = tft_store::health(&tft_store::open(&path).unwrap()).unwrap().last_error.expect("a logged problem");
        assert_eq!((last.region.as_str(), last.level.as_str()), ("", WARN));
        assert!(last.message.starts_with("Could not download the patch calendar"), "{}", last.message);
    }
}
