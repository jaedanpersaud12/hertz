//! The side-by-side experiment of PRD 2 C1 (`tft comps-report`): today's rule and the clusters method on the
//! same boards of one patch and rank scope, top comps of each next to each other, with the two measures the
//! PRD judges them by: coverage (share of top 4 boards that land in a listed comp) and separation (the most
//! core overlap between two listed comps). Your renames and merges are left out: this compares the rules.
//! Nothing is written; the work happens in a transaction that is rolled back.

use crate::cluster::{self, Acc, Boards, overlap};
use crate::{COMP_LIMIT, COMP_MIN_GAMES, COMP_MIN_SHARE, merge_lookalikes, ranked_only, recent_patches};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::time::Instant;
use tft_core::config::{BUCKETS, scope_key};

pub struct ReportComp {
    pub key: String,
    pub name: String,
    pub games: i64,
    pub avg_place: f64,
    pub core: Vec<String>,
}

pub struct MethodReport {
    pub comps: Vec<ReportComp>,
    /// Top 4 boards in a listed comp, and all top 4 boards.
    pub covered: i64,
    pub top4: i64,
    /// The two listed comps whose cores overlap most, and how much.
    pub max_overlap: Option<(f64, String, String)>,
    pub seconds: f64,
}

impl MethodReport {
    pub fn coverage(&self) -> f64 {
        if self.top4 == 0 { 0.0 } else { self.covered as f64 / self.top4 as f64 }
    }
}

pub struct Report {
    pub patch: String,
    pub scope: String,
    pub boards: i64,
    pub min_games: i64,
    pub baseline: MethodReport,
    pub clusters: MethodReport,
}

/// Runs both methods on one patch (default: the newest) and rank bucket (default: all ranks), whole patch.
pub fn comps_report(conn: &mut Connection, patch: Option<&str>, bucket: Option<&str>) -> Result<Report> {
    let patch = match patch {
        Some(p) => p.to_string(),
        None => match recent_patches(conn, 1)?.pop() {
            Some(p) => p,
            None => bail!("No matches stored yet. Run \"tft crawl\" first."),
        },
    };
    let b = match bucket {
        None => &BUCKETS[0],
        Some(k) => match BUCKETS.iter().find(|b| b.key.eq_ignore_ascii_case(k)) {
            Some(b) => b,
            None => bail!(
                "Unknown rank bucket {k}. Use one of: {}.",
                BUCKETS.iter().map(|b| b.key).collect::<Vec<_>>().join(", ")
            ),
        },
    };
    let scope = scope_key(b.key, "");
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
         DELETE FROM p;",
    )?;
    tx.execute(
        &format!(
            "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
             JOIN matches m ON m.match_id = pa.match_id WHERE m.patch = ? AND m.rank >= ? AND {}",
            ranked_only("m")
        ),
        params![patch, b.min],
    )?;
    let rows: Vec<(i64, i64, Option<i64>, Option<String>)> = tx
        .prepare("SELECT id, placement, level, comp_key FROM p ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let n = rows.len() as i64;
    if n == 0 {
        bail!("No boards in patch {patch} at {}.", b.key);
    }
    let min_games = COMP_MIN_GAMES.max(((n as f64) * COMP_MIN_SHARE).ceil() as i64);
    let boards = Boards::load(&tx, "p")?;

    let t = Instant::now();
    merge_lookalikes(&tx, min_games)?;
    let mut baseline = summarise(&tx, &boards, min_games)?;
    baseline.seconds = t.elapsed().as_secs_f64();

    tx.execute("DELETE FROM p", [])?;
    {
        let mut put = tx.prepare("INSERT INTO p VALUES (?, ?, ?, ?)")?;
        for (id, place, level, key) in &rows {
            put.execute(params![id, place, level, key])?;
        }
    }
    let t = Instant::now();
    cluster::label(&tx, min_games, COMP_LIMIT)?;
    let mut clusters = summarise(&tx, &boards, min_games)?;
    clusters.seconds = t.elapsed().as_secs_f64();
    tx.rollback()?;
    Ok(Report { patch, scope, boards: n, min_games, baseline, clusters })
}

/// The listed comps of the labels now in p, as the stats would list them, with names, cores and the measures.
fn summarise(db: &Connection, boards: &Boards, min_games: i64) -> Result<MethodReport> {
    let listed: Vec<(String, i64, i64)> = db
        .prepare(
            "SELECT comp_key, COUNT(*), SUM(placement) FROM p WHERE comp_key IS NOT NULL GROUP BY comp_key
             HAVING COUNT(*) >= ? ORDER BY COUNT(*) DESC, comp_key LIMIT ?",
        )?
        .query_map(params![min_games, COMP_LIMIT], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<&str, usize> = listed.iter().enumerate().map(|(i, c)| (c.0.as_str(), i)).collect();
    let d = boards.names.len();
    let mut accs: Vec<Acc> = vec![Acc::new(d); listed.len()];
    let mut full: Vec<Vec<u32>> = vec![vec![0; d]; listed.len()];
    let (mut covered, mut top4) = (0, 0);
    {
        let mut st = db.prepare("SELECT id, placement, comp_key FROM p ORDER BY id")?;
        let mut rs = st.query([])?;
        while let Some(r) = rs.next()? {
            let (id, place, key): (i64, i64, Option<String>) = (r.get(0)?, r.get(1)?, r.get(2)?);
            let c = key.as_deref().and_then(|k| index.get(k)).copied();
            if place <= 4 {
                top4 += 1;
                covered += i64::from(c.is_some());
            }
            if let (Some(c), Some(i)) = (c, boards.find(id)) {
                accs[c].add_board(boards, i);
                for k in boards.units(i) {
                    if boards.items[k] >= 3 {
                        full[c][boards.unit[k] as usize] += 1;
                    }
                }
            }
        }
    }

    // Top trait by unit count among traits on at least half the boards (at its most common level).
    let mut traits: HashMap<String, Vec<(String, i64, i64)>> = HashMap::new(); // comp -> (trait, games, units)
    {
        let mut st = db.prepare(
            "SELECT p.comp_key, t.name, t.tier_current, COUNT(*), MIN(t.num_units) FROM p JOIN traits t ON t.participant_id = p.id
             WHERE p.comp_key IS NOT NULL GROUP BY p.comp_key, t.name, t.tier_current",
        )?;
        let mut rs = st.query([])?;
        while let Some(r) = rs.next()? {
            let key: String = r.get(0)?;
            if index.contains_key(key.as_str()) {
                traits.entry(key).or_default().push((r.get(1)?, r.get(3)?, r.get(4)?));
            }
        }
    }
    let display = |kind: &str, id: &str| -> Result<String> {
        Ok(db
            .query_row("SELECT name FROM static_data WHERE kind = ? AND id = ?", [kind, id], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| id.to_string()))
    };

    let mut comps = Vec::new();
    for (c, (key, games, sum_place)) in listed.iter().enumerate() {
        let g = *games as f64;
        let mut by: HashMap<&str, (i64, i64, i64)> = HashMap::new(); // trait -> (total games, best level games, units)
        for (name, tg, units) in traits.get(key).into_iter().flatten() {
            let e = by.entry(name.as_str()).or_insert((0, -1, 0));
            e.0 += tg;
            if *tg > e.1 {
                (e.1, e.2) = (*tg, *units);
            }
        }
        let mut top: Vec<(&str, i64, i64)> =
            by.into_iter().filter(|(_, v)| v.0 as f64 / g >= 0.5).map(|(n, v)| (n, v.2, v.0)).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(b.0)));
        let mut name = vec![match top.first() {
            Some(t) => display("trait", t.0)?,
            None => "Flex".to_string(),
        }];
        let mut carries: Vec<usize> = (0..d).filter(|u| full[c][*u] as f64 / g >= 0.4).collect();
        carries.sort_by(|a, b| full[c][*b].cmp(&full[c][*a]).then(a.cmp(b)));
        if carries.is_empty() {
            carries.extend((0..d).filter(|u| full[c][*u] > 0).max_by(|a, b| full[c][*a].cmp(&full[c][*b]).then(b.cmp(a))));
        }
        for u in carries.into_iter().take(2) {
            name.push(display("unit", &boards.names[u])?);
        }
        comps.push(ReportComp {
            key: key.clone(),
            name: name.join(" "),
            games: *games,
            avg_place: *sum_place as f64 / g,
            core: accs[c].core_key(&boards.names),
        });
    }
    let mut max_overlap: Option<(f64, String, String)> = None;
    for i in 0..comps.len() {
        for j in i + 1..comps.len() {
            let o = overlap(&comps[i].core, &comps[j].core);
            if max_overlap.as_ref().is_none_or(|m| o > m.0) {
                max_overlap = Some((o, comps[i].name.clone(), comps[j].name.clone()));
            }
        }
    }
    Ok(MethodReport { comps, covered, top4, max_overlap, seconds: 0.0 })
}

fn cut(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w { format!("{s:<w$}") } else { format!("{}~", s.chars().take(w - 1).collect::<String>()) }
}

/// The report as text: the top `top` comps of each method side by side, then the measures.
pub fn format_report(r: &Report, top: usize) -> String {
    let w = 30;
    let mut out = format!(
        "Comps report: patch {}, {} (whole patch), {} boards; a comp is listed from {} games.\n\n",
        r.patch, r.scope, r.boards, r.min_games
    );
    out.push_str(&format!("  #  {:<w$} {:>7} {:>5}   {:<w$} {:>7} {:>5}\n", "Today's rule", "games", "avg", "Clusters", "games", "avg"));
    let side = |c: Option<&ReportComp>| match c {
        Some(c) => format!("{} {:>7} {:>5.2}", cut(&c.name, w), c.games, c.avg_place),
        None => format!("{:<w$} {:>7} {:>5}", "", "", ""),
    };
    for i in 0..top.min(r.baseline.comps.len().max(r.clusters.comps.len())) {
        out.push_str(&format!("{:>3}  {}   {}\n", i + 1, side(r.baseline.comps.get(i)), side(r.clusters.comps.get(i))));
    }
    let pct = |x: f64| format!("{:.1}%", x * 100.0);
    let line = |label: &str, a: String, b: String| format!("{label:<46}{a:<30}{b}\n");
    out.push('\n');
    out.push_str(&line("", "Today's rule".into(), "Clusters".into()));
    out.push_str(&line("Listed comps", r.baseline.comps.len().to_string(), r.clusters.comps.len().to_string()));
    out.push_str(&line(
        "Coverage (top 4 boards in a listed comp)",
        format!("{} ({}/{})", pct(r.baseline.coverage()), r.baseline.covered, r.baseline.top4),
        format!("{} ({}/{})", pct(r.clusters.coverage()), r.clusters.covered, r.clusters.top4),
    ));
    let worst = |m: &MethodReport| m.max_overlap.as_ref().map_or("-".to_string(), |(o, ..)| pct(*o));
    out.push_str(&line("Most core overlap between two listed comps", worst(&r.baseline), worst(&r.clusters)));
    out.push_str(&line(
        "Time to label the boards",
        format!("{:.2} s", r.baseline.seconds),
        format!("{:.2} s", r.clusters.seconds),
    ));
    for (label, m) in [("Today's rule", &r.baseline), ("Clusters", &r.clusters)] {
        if let Some((o, a, b)) = &m.max_overlap {
            out.push_str(&format!("{label}: most overlapping pair is \"{a}\" and \"{b}\" ({}).\n", pct(*o)));
        }
    }
    out.push_str("Targets (PRD 2): coverage 90% or more; core overlap under 75%; 27 of the top 30 names fine as they are.\n");
    out
}
