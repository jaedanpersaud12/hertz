//! `tft stats --profile`: how long each SQL statement of the stats build took, summed over every scope, so a
//! slow step shows up by name. SQLite reports each statement's run time; time spent in Rust between
//! statements is the difference between the wall time and the SQL total.

use rusqlite::Connection;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Row {
    runs: u64,
    total: Duration,
    max: Duration,
}

pub struct Profile {
    rows: Arc<Mutex<HashMap<String, Row>>>,
    started: Instant,
}

/// One line per statement shape: whitespace folded, cut to 150 characters.
fn shape(sql: &str) -> String {
    let flat: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(150).collect()
}

impl Profile {
    pub fn start(conn: &mut Connection) -> rusqlite::Result<Profile> {
        let rows: Arc<Mutex<HashMap<String, Row>>> = Arc::default();
        let sink = rows.clone();
        conn.trace_v2(
            TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(move |e: TraceEvent<'_>| {
                if let TraceEvent::Profile(stmt, took) = e {
                    let mut rows = sink.lock().unwrap();
                    let row = rows.entry(shape(&stmt.sql())).or_default();
                    row.runs += 1;
                    row.total += took;
                    row.max = row.max.max(took);
                }
            }),
        )?;
        Ok(Profile { rows, started: Instant::now() })
    }

    /// Prints the slowest statements, slowest first, and how much of the wall time was SQL.
    pub fn report(&self, top: usize) {
        let wall = self.started.elapsed();
        let rows = self.rows.lock().unwrap();
        let mut list: Vec<(&String, &Row)> = rows.iter().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.1.total));
        let sql: Duration = list.iter().map(|(_, r)| r.total).sum();
        println!();
        println!(
            "Profile: {:.1} s wall, {:.1} s in SQL, {:.1} s elsewhere (Rust). Slowest statements, summed over every scope:",
            wall.as_secs_f64(),
            sql.as_secs_f64(),
            wall.saturating_sub(sql).as_secs_f64()
        );
        println!("{:>9} {:>6} {:>8} {:>8}  statement", "total s", "share", "runs", "max s");
        for (text, r) in list.iter().take(top) {
            println!(
                "{:>9.1} {:>5.0}% {:>8} {:>8.1}  {}",
                r.total.as_secs_f64(),
                100.0 * r.total.as_secs_f64() / wall.as_secs_f64().max(1e-9),
                r.runs,
                r.max.as_secs_f64(),
                text
            );
        }
    }
}
