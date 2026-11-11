//! `tft`: the Hertz engine without the app, in one program.
//!
//!   tft run [--port N] [--dev] serve the app and keep collecting matches, in one process (Ctrl+C stops both)
//!   tft crawl                  collect matches until Ctrl+C, rebuilding the stats every 10 minutes when new ones arrive
//!   tft crawl --once           stop when every known player is up to date, then build the stats once
//!   tft crawl --no-supervisor  collect matches only: no stats builds, data refresh or pruning
//!   tft ingest FILE...         store saved match JSON files (one match, or an array of matches, per file)
//!   tft patches                download the patch calendar and date matches stored without one
//!   tft stats [--patches N]    rebuild the stat tables for the N most recent patches (same as `npm run stats`)
//!   tft stats --incremental    rebuild only the parts with new matches, and list them
//!   tft serve [--port N] [--dev]  serve the app and its API on 127.0.0.1 (PORT from .env, then 5174)
//!                              --dev: the API alone on PORT+1 for Vite, with the ?data= stress switch
//!   tft refresh                download the patch calendar and the set data (names, costs, icons) now
//!   tft recluster              label every stored board with its comp again, after the comp rules change
//!   tft health                 matches per hour per server, matches waiting, the last problem, how old the stats are
//!   tft init                   create or upgrade the database and print where it is
//!
//! Settings come from `.env` in the current folder, the same file the TypeScript engine reads.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tft_core::config::{Config, tier_rank};
use tft_core::riot::RiotMatch;
use std::time::Duration;
use tft_riot::cdragon::Source;
use tft_riot::{CrawlOptions, Crawler, KeyProblem, RiotError, SupervisorOptions, Timings, run_supervisor};
use tft_store::{Ingest, load_calendar, relabel_unknown, save_calendar};

#[derive(Parser)]
#[command(
    name = "tft",
    version,
    about = "The Hertz engine: collects TFT matches from the Riot API, builds the stats and serves the app.",
    after_help = "Settings come from .env in the current folder. Start with `tft run` to do everything at once."
)]
struct Cli {
    /// Database file. Defaults to DB_PATH from .env, then data/tft.db.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the app and keep collecting matches in the same process. Ctrl+C stops both.
    ///
    /// The stats are rebuilt every 10 minutes when new matches arrived. Without a Riot API key the app is still
    /// served; the crawl waits until you add the key and start again.
    Run {
        /// Port to listen on. Defaults to PORT from .env, then 5174 (one more with --dev).
        #[arg(long)]
        port: Option<u16>,
        /// Refresh the player list from the ladders first.
        #[arg(long)]
        reseed: bool,
        /// For `npm run dev`: the API alone on PORT+1, behind Vite, with the `?data=` stress switch.
        #[arg(long)]
        dev: bool,
    },
    /// Collect ranked matches from the Riot API until Ctrl+C.
    ///
    /// Also rebuilds the stats every 10 minutes when new matches arrived, downloads the patch calendar and set
    /// data every 3 hours, and prunes old raw matches once a day.
    Crawl {
        /// Stop after storing this many matches.
        #[arg(long)]
        max: Option<u64>,
        /// Refresh the player list from the ladders first.
        #[arg(long)]
        reseed: bool,
        /// Stop when every known player is up to date, then build the stats once.
        #[arg(long)]
        once: bool,
        /// Only collect matches: no stats builds, data refresh or pruning while crawling.
        #[arg(long)]
        no_supervisor: bool,
    },
    /// Store saved match files, as if the crawler had fetched them.
    Ingest {
        /// Match JSON files: one match, or an array of matches, per file.
        files: Vec<PathBuf>,
        /// Server the matches were found on.
        #[arg(long, default_value = "na1")]
        platform: String,
        /// Tier of the player they were found through.
        #[arg(long, default_value = "CHALLENGER")]
        tier: String,
        /// Do not add the players in these matches to the crawl list.
        #[arg(long)]
        no_snowball: bool,
    },
    /// Download the patch calendar and date matches stored before it was known.
    Patches,
    /// Rebuild the stat tables the app reads.
    Stats {
        /// How many of the most recent patches to rebuild.
        #[arg(long, default_value_t = 2)]
        patches: i64,
        /// Rebuild only the ranks and time windows that have new matches, and list them. Gives the same tables.
        #[arg(long)]
        incremental: bool,
    },
    /// Serve the built app and its API on 127.0.0.1, like `npm start`.
    Serve {
        /// Port to listen on. Defaults to PORT from .env, then 5174 (one more with --dev).
        #[arg(long)]
        port: Option<u16>,
        /// For `npm run dev`: the API alone on PORT+1, behind Vite, with the `?data=` stress switch.
        #[arg(long)]
        dev: bool,
    },
    /// Download the patch calendar and the set data (names, costs, icons, recipes) from Community Dragon now.
    ///
    /// `tft crawl` and `tft run` also do this at start and every 3 hours. CDRAGON_BASE in .env points it at
    /// another address (tests use a fake one).
    Refresh,
    /// Label every stored board with its comp again, using the current comp rules.
    ///
    /// Run it after changing the rules in tft-core's cluster.rs, or once on a database made before comps existed,
    /// then `tft stats`.
    Recluster,
    /// Show how the crawl is doing: matches per hour per server, matches waiting, the last problem, stats age.
    Health,
    /// Create or upgrade the database.
    Init,
}

fn main() {
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    let mut config = Config::from_env();
    if let Some(db) = cli.db {
        config.db_path = db;
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(err) = runtime.block_on(run(cli.command, config)) {
        // Key problems get their plain message; anything else gets the full chain.
        match err.downcast_ref::<RiotError>() {
            Some(RiotError::Key(msg)) => eprintln!("\n{msg}"),
            _ => eprintln!("Error: {err:#}"),
        }
        std::process::exit(1);
    }
}

async fn run(command: Command, config: Config) -> Result<()> {
    match command {
        Command::Init => {
            tft_store::open(&config.db_path)?;
            println!("Database ready at {}", config.db_path.display());
        }

        Command::Serve { port, dev } => {
            let (port, db_path) = (port.unwrap_or(default_port(&config, dev)), config.db_path.clone());
            tft_api::serve(tft_api::Options { port, db_path, config, dev }).await?;
        }

        Command::Crawl { max, reseed, once, no_supervisor } => {
            let stop = stop_on_ctrl_c();
            let conn = tft_store::open(&config.db_path)?;
            let db_path = config.db_path.clone();
            let crawler = Crawler::new(config, CrawlOptions { max, reseed, once }, conn, Arc::clone(&stop))?;
            if no_supervisor {
                crawler.run().await?;
            } else {
                run_supervisor(crawler, supervisor_options(&db_path)).await?;
                // Stopped by --once or --max, not by Ctrl+C: leave the stats up to date with what was stored.
                if !stop.load(Ordering::Relaxed) {
                    tokio::task::spawn_blocking(move || {
                        tft_riot::supervisor::build_stats_step(&mut tft_store::open(&db_path)?, tft_stats::DEFAULT_PATCHES)
                    })
                    .await??;
                }
            }
        }

        Command::Run { port, reseed, dev } => {
            let stop = stop_on_ctrl_c();
            let db_path = config.db_path.clone();
            // Opened once before both halves start, so they never race to create or upgrade the tables.
            let conn = tft_store::open(&db_path)?;
            let opts = tft_api::Options {
                port: port.unwrap_or(default_port(&config, dev)),
                db_path: db_path.clone(),
                config: config.clone(),
                dev,
            };
            let mut server = tokio::spawn(tft_api::serve(opts));
            let key_missing = config.api_key.trim().is_empty();
            let crawler = Crawler::new(config, CrawlOptions { reseed, ..Default::default() }, conn, Arc::clone(&stop))?;
            let mut crawl = tokio::spawn(async move {
                if key_missing {
                    // The crawl would only fail on its first request; say why once and keep the app up.
                    return Err(RiotError::Key(KeyProblem::Missing).into());
                }
                run_supervisor(crawler, supervisor_options(&db_path)).await
            });
            let mut crawling = true;
            loop {
                tokio::select! {
                    served = &mut server => {
                        stop.store(true, Ordering::Relaxed);
                        served??;
                        bail!("The server stopped on its own.");
                    }
                    done = &mut crawl, if crawling => {
                        crawling = false;
                        crawl_ended(done?);
                    }
                    _ = stopped(&stop) => break,
                }
            }
            // Ctrl+C: the crawler finishes the requests in flight, then the server stops taking requests.
            if crawling {
                crawl_ended(crawl.await?);
            }
            server.abort();
            let _ = server.await;
            println!("Stopped.");
        }

        Command::Health => {
            let conn = tft_store::open(&config.db_path)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
            for line in health_lines(&tft_store::health(&conn)?, now) {
                println!("{line}");
            }
        }

        Command::Ingest { files, platform, tier, no_snowball } => {
            if files.is_empty() {
                bail!("Give one or more match files.");
            }
            let mut conn = tft_store::open(&config.db_path)?;
            let calendar = load_calendar(&conn)?;
            let tier = tier.to_uppercase();
            let ingest = Ingest { platform: &platform, rank: tier_rank(&tier), tier: &tier, snowball: !no_snowball };
            let (mut added, mut repeats) = (0, 0);
            for file in &files {
                let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
                let value: serde_json::Value =
                    serde_json::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
                let list = match value {
                    serde_json::Value::Array(xs) => xs,
                    one => vec![one],
                };
                for v in list {
                    let m: RiotMatch = serde_json::from_value(v).with_context(|| format!("a match in {}", file.display()))?;
                    if ingest.store(&mut conn, &m, &calendar)? {
                        added += 1;
                    } else {
                        repeats += 1;
                    }
                }
            }
            println!("Stored {added} matches ({repeats} were already stored).");
        }

        Command::Stats { patches, incremental } => {
            let mut conn = tft_store::open(&config.db_path)?;
            // Matches stored without a patch (Riot stopped sending one) get dated from the patch calendar first.
            let unknown: i64 =
                conn.query_row("SELECT COUNT(*) n FROM matches WHERE patch = 'unknown'", [], |r| r.get(0))?;
            if unknown > 0 {
                let cal = match tft_riot::cdragon::fetch_calendar(&tft_riot::http_client()).await {
                    Ok(cal) if !cal.is_empty() => {
                        save_calendar(&conn, &cal)?;
                        cal
                    }
                    _ => load_calendar(&conn)?,
                };
                let fixed = relabel_unknown(&mut conn, &cal)?;
                if fixed > 0 {
                    println!("Dated {} matches to their patch.", thousands(fixed));
                } else {
                    println!("Could not reach Community Dragon for the patch calendar. Check your connection and run this again.");
                }
            }
            if incremental {
                let touched = tft_stats::build_stats_incremental_with(&mut conn, patches, &mut |line| println!("{line}"))?;
                if !touched.is_empty() {
                    println!("Rebuilt {} scopes: {}", touched.len(), touched.join(", "));
                }
            } else {
                tft_stats::build_stats(&mut conn, patches)?;
            }
        }

        Command::Refresh => {
            let mut conn = tft_store::open(&config.db_path)?;
            let src = cdragon_source();
            let r = tft_riot::cdragon::refresh(&mut conn, &src)
                .await
                .context("Could not download the patch calendar from Community Dragon. Check your connection and run this again")?;
            let cal = load_calendar(&conn)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
            println!("Patch calendar saved. Current patch: {}.", cal.patch_at(now));
            if let Some(p) = &r.new_patch {
                println!("New patch: {p}.");
            }
            if r.relabelled > 0 {
                println!("Dated {} matches to their patch.", thousands(r.relabelled));
            }
            match (&r.static_set, &r.static_error) {
                (Some(s), _) => println!(
                    "Set data saved: set {} ({}), {} units, {} traits, {} items.",
                    s.set_number, s.set_name, s.units, s.traits, s.items
                ),
                (None, Some(e)) => bail!("Could not download the set data: {e}"),
                (None, None) => println!("Set data is already up to date."),
            }
        }

        Command::Recluster => {
            let mut conn = tft_store::open(&config.db_path)?;
            recluster(&mut conn, &mut |line| println!("{line}"))?;
            println!("Done. Run \"npm run stats\" to rebuild the numbers.");
        }

        Command::Patches => {
            let mut conn = tft_store::open(&config.db_path)?;
            let cal = match tft_riot::cdragon::fetch_calendar(&tft_riot::http_client()).await {
                Ok(cal) => {
                    save_calendar(&conn, &cal)?;
                    cal
                }
                Err(e) => {
                    println!("Could not download the patch calendar ({e:#}). Using the saved one.");
                    load_calendar(&conn)?
                }
            };
            if cal.is_empty() {
                bail!("No patch calendar yet. Check your connection and run this again.");
            }
            let fixed = relabel_unknown(&mut conn, &cal)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
            println!("Current patch: {}. Dated {fixed} matches to their patch.", cal.patch_at(now));
        }
    }
    Ok(())
}

/// PORT from .env (then 5174); with `--dev` the one after it, since Vite takes PORT and proxies `/api` there.
fn default_port(config: &Config, dev: bool) -> u16 {
    if dev { config.port.saturating_add(1) } else { config.port }
}

/// Sets the returned flag on the first Ctrl+C so the crawler finishes what is in flight; a second one exits at once.
fn stop_on_ctrl_c() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            if flag.swap(true, Ordering::Relaxed) {
                std::process::exit(1);
            }
            println!("\nFinishing requests in flight, then stopping...");
        }
    });
    stop
}

/// Resolves once `stop` is set.
async fn stopped(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Community Dragon, or CDRAGON_BASE from .env when it is set.
fn cdragon_source() -> Source {
    let mut src = Source::live();
    if let Ok(base) = std::env::var("CDRAGON_BASE")
        && !base.trim().is_empty()
    {
        src.base = base.trim().trim_end_matches('/').to_string();
    }
    src
}

/// Gives every stored board the comp label the current rules give it, in steps of 20,000 boards, as
/// `src/stats/recluster.ts` did. Boards with no units keep the label they have.
fn recluster(conn: &mut rusqlite::Connection, log: &mut dyn FnMut(String)) -> Result<()> {
    use tft_core::cluster::{KeyTrait, KeyUnit, comp_key};
    let total: i64 = conn.query_row("SELECT COUNT(*) n FROM participants", [], |r| r.get(0))?;
    let max_id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) n FROM participants", [], |r| r.get(0))?;
    const STEP: i64 = 20_000;
    let mut done = 0usize;
    let mut from = 1;
    while from <= max_id {
        let to = from + STEP - 1;
        // Boards in the order their first unit was stored, each with its units and traits in stored order.
        let mut order: Vec<i64> = Vec::new();
        type Unit = (String, i64, Option<i64>, i64);
        type Trait = (String, i64, i64, i64);
        let mut units: std::collections::HashMap<i64, Vec<Unit>> = Default::default();
        let mut traits: std::collections::HashMap<i64, Vec<Trait>> = Default::default();
        {
            let mut q = conn.prepare_cached(
                "SELECT participant_id, character_id, star, rarity, n_items FROM units WHERE participant_id BETWEEN ? AND ?",
            )?;
            let mut rows = q.query([from, to])?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                let list = units.entry(id).or_insert_with(|| {
                    order.push(id);
                    Vec::new()
                });
                list.push((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?));
            }
            let mut q = conn.prepare_cached(
                "SELECT participant_id, name, num_units, tier_current, style FROM traits WHERE participant_id BETWEEN ? AND ?",
            )?;
            let mut rows = q.query([from, to])?;
            while let Some(r) = rows.next()? {
                traits.entry(r.get(0)?).or_default().push((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?));
            }
        }
        let tx = conn.transaction()?;
        {
            let mut update = tx.prepare_cached("UPDATE participants SET comp_key = ? WHERE id = ?")?;
            for id in &order {
                let us: Vec<KeyUnit> = units[id]
                    .iter()
                    .map(|(c, star, rarity, n)| KeyUnit { character_id: c, star: *star, rarity: *rarity, n_items: *n })
                    .collect();
                let ts: Vec<KeyTrait> = traits
                    .get(id)
                    .map(|v| {
                        v.iter()
                            .map(|(name, n, tier, style)| KeyTrait { name, num_units: *n, tier_current: *tier, style: *style })
                            .collect()
                    })
                    .unwrap_or_default();
                update.execute(rusqlite::params![comp_key(&us, &ts), id])?;
            }
        }
        tx.commit()?;
        done += order.len();
        log(format!("{} of {} boards", thousands(done), thousands(total as usize)));
        from += STEP;
    }
    Ok(())
}

fn supervisor_options(db_path: &std::path::Path) -> SupervisorOptions {
    SupervisorOptions {
        db_path: db_path.to_path_buf(),
        patch_count: tft_stats::DEFAULT_PATCHES,
        source: cdragon_source(),
        timings: Timings::default(),
    }
}

/// `tft run`: the crawl has ended while the app keeps being served. Says why, in the crawler's words.
fn crawl_ended(result: Result<()>) {
    match result {
        Ok(()) => println!("The crawl stopped. The app is still being served; press Ctrl+C to stop it."),
        Err(err) => {
            match err.downcast_ref::<RiotError>() {
                Some(RiotError::Key(msg)) => eprintln!("\n{msg}"),
                _ => eprintln!("The crawl stopped: {err:#}"),
            }
            eprintln!("The app is still being served without new matches. Press Ctrl+C to stop it.");
        }
    }
}

/// `tft health` as plain lines.
fn health_lines(h: &tft_store::Health, now: i64) -> Vec<String> {
    let ago = |at: i64| {
        let mins = (now - at).max(0) / 60_000;
        match mins {
            0 => "just now".to_string(),
            1 => "1 minute ago".to_string(),
            m if m < 120 => format!("{m} minutes ago"),
            m => format!("{} hours ago", m / 60),
        }
    };
    let mut out = Vec::new();
    if h.per_region.is_empty() {
        out.push("Matches stored: none in the last 24 hours.".to_string());
    } else {
        out.push("Matches stored, per server:".to_string());
        for r in &h.per_region {
            let day: i64 = r.matches_last_24h_per_hour.iter().sum();
            out.push(format!("  {:<6} {} in the last hour, {} in the last 24 hours", r.region, r.matches_last_hour, day));
        }
    }
    if h.queue_depth_per_region.is_empty() {
        out.push("Matches waiting to be fetched: 0".to_string());
    } else {
        let total: i64 = h.queue_depth_per_region.iter().map(|q| q.pending).sum();
        let each: Vec<String> = h.queue_depth_per_region.iter().map(|q| format!("{} {}", q.region, q.pending)).collect();
        out.push(format!("Matches waiting to be fetched: {total} ({})", each.join(", ")));
    }
    match &h.last_error {
        None => out.push("Last problem: none.".to_string()),
        Some(e) => {
            let region = if e.region.is_empty() { String::new() } else { format!(" on {}", e.region) };
            out.push(format!("Last problem ({}{region}, {}): {}", e.level, ago(e.at), e.message));
        }
    }
    match (h.stats_built_at, h.stats_age_minutes) {
        (Some(at), Some(mins)) => out.push(format!("Stats built {} ({mins} minutes old).", ago(at))),
        _ => out.push("Stats: not built yet. Run `tft stats` or keep `tft crawl` running.".to_string()),
    }
    match h.newest_match_at {
        Some(at) => out.push(format!("Newest match stored {}.", ago(at))),
        None => out.push("Newest match stored: none yet.".to_string()),
    }
    out
}

/// 1234567 as "1,234,567", as `toLocaleString('en-US')` prints it.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
