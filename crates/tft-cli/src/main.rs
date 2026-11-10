//! `tft`: the engine without the app.
//!
//!   tft crawl                  collect matches until Ctrl+C (same as `npm run crawl`)
//!   tft crawl --once           stop when every known player is up to date
//!   tft ingest FILE...         store saved match JSON files (one match, or an array of matches, per file)
//!   tft patches                download the patch calendar and date matches stored without one
//!   tft stats [--patches N]    rebuild the stat tables for the N most recent patches (same as `npm run stats`)
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
use tft_riot::{CrawlOptions, Crawler, RiotError};
use tft_store::{Ingest, load_calendar, relabel_unknown, save_calendar};

#[derive(Parser)]
#[command(name = "tft", version, about = "TFT Local engine")]
struct Cli {
    /// Database file. Defaults to DB_PATH from .env, then data/tft.db.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Collect ranked matches from the Riot API.
    Crawl {
        /// Stop after storing this many matches.
        #[arg(long)]
        max: Option<u64>,
        /// Refresh the player list from the ladders first.
        #[arg(long)]
        reseed: bool,
        /// Stop when every known player is up to date.
        #[arg(long)]
        once: bool,
    },
    /// Store saved match files, as if the crawler had fetched them.
    Ingest {
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
    },
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

        Command::Crawl { max, reseed, once } => {
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
            let conn = tft_store::open(&config.db_path)?;
            let crawler = Crawler::new(config, CrawlOptions { max, reseed, once }, conn, stop)?;
            crawler.run().await?;
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

        Command::Stats { patches } => {
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
            tft_stats::build_stats(&mut conn, patches)?;
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
