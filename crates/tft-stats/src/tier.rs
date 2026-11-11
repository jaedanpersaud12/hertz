//! Tier letters and trends for the comp tier list (PRD 2 C3, C4). The weights, cut-offs and the "rising" line
//! are opinion, so they live in `config/tier-list.json`, not here.
//!
//! Per scope (patch, rank filter, time window), over the comps that scope lists:
//!
//! ```text
//! score      = 0.55 * z(avg_place, inverted) + 0.25 * z(top4_rate) + 0.20 * z(win_rate)
//! confidence = games / (games + 400)
//! final      = score * confidence
//! ```
//!
//! z is the standard score within the scope (population standard deviation; 0 when every comp has the same
//! value). Comps are ranked by final score (ties: more games first, then comp key), and the tiers take their
//! share of that ranking in order: S the top 8%, A the next 17%, and so on, the last tier the rest. A share is
//! rounded to the nearest comp, on the running total, so a scope with one comp puts it in B.
//!
//! Trends: `prev_place` is the comp's average place in the previous patch (same rank filter, whole patch), and
//! `d3_place` its average place in the last 3 days of this patch (whole-patch rows only). Both are NULL when
//! that comp has no row there.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::cmp::Ordering;
use std::sync::OnceLock;

/// The tier list settings file, read when the engine is built.
pub const CONFIG_JSON: &str = include_str!("../../../config/tier-list.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Weights {
    pub avg_place: f64,
    pub top4_rate: f64,
    pub win_rate: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TierCut {
    pub tier: String,
    /// Share of the listed comps, in whole percent.
    pub percent: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TierConfig {
    pub weights: Weights,
    /// The games at which confidence is one half.
    pub confidence_games: f64,
    pub tiers: Vec<TierCut>,
    /// A comp is rising when it places at least this much better than last patch, or than the whole patch over
    /// the last 3 days.
    pub rising_places: f64,
}

/// The settings in `config/tier-list.json`.
pub fn config() -> &'static TierConfig {
    static CONFIG: OnceLock<TierConfig> = OnceLock::new();
    CONFIG.get_or_init(|| serde_json::from_str(CONFIG_JSON).expect("config/tier-list.json is not valid"))
}

/// One comp's sums in a scope.
#[derive(Debug, Clone)]
pub struct CompSums {
    pub key: String,
    pub games: i64,
    pub sum_place: i64,
    pub wins: i64,
    pub top4: i64,
}

/// Standard scores of `xs`: (x - mean) / population standard deviation, or 0 for all when they do not vary.
pub fn z_scores(xs: &[f64]) -> Vec<f64> {
    let n = xs.len() as f64;
    if xs.is_empty() {
        return Vec::new();
    }
    let mean = xs.iter().sum::<f64>() / n;
    let sd = (xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n).sqrt();
    if sd.is_nan() || sd <= 1e-12 {
        return vec![0.0; xs.len()];
    }
    xs.iter().map(|x| (x - mean) / sd).collect()
}

/// `games / (games + confidence_games)`.
pub fn confidence(games: i64, cfg: &TierConfig) -> f64 {
    let g = games as f64;
    g / (g + cfg.confidence_games)
}

/// Final score and tier letter per comp, in the order given.
pub fn grade(comps: &[CompSums], cfg: &TierConfig) -> Vec<(f64, String)> {
    let rate = |f: fn(&CompSums) -> i64| -> Vec<f64> { comps.iter().map(|c| f(c) as f64 / c.games as f64).collect() };
    let place = z_scores(&rate(|c| c.sum_place));
    let top4 = z_scores(&rate(|c| c.top4));
    let wins = z_scores(&rate(|c| c.wins));
    let w = &cfg.weights;
    let finals: Vec<f64> = (0..comps.len())
        .map(|i| (w.avg_place * -place[i] + w.top4_rate * top4[i] + w.win_rate * wins[i]) * confidence(comps[i].games, cfg))
        .collect();

    let mut order: Vec<usize> = (0..comps.len()).collect();
    order.sort_by(|&a, &b| {
        finals[b]
            .partial_cmp(&finals[a])
            .unwrap_or(Ordering::Equal)
            .then(comps[b].games.cmp(&comps[a].games))
            .then(comps[a].key.cmp(&comps[b].key))
    });
    let n = comps.len() as i64;
    let mut cuts = Vec::new(); // (comps ranked above this line, tier)
    let mut running = 0;
    for (i, t) in cfg.tiers.iter().enumerate() {
        running += t.percent;
        let line = if i + 1 == cfg.tiers.len() { n } else { (n * running + 50) / 100 };
        cuts.push((line, t.tier.as_str()));
    }
    let mut out = vec![(0.0, String::new()); comps.len()];
    for (rank, &i) in order.iter().enumerate() {
        let tier = cuts.iter().find(|(line, _)| (rank as i64) < *line).map_or("", |(_, t)| t);
        out[i] = (finals[i], tier.to_string());
    }
    out
}

/// Whether a comp counts as rising: its average place is at least `rising_places` better (lower) than last
/// patch, or its last 3 days are at least that much better than its whole patch.
pub fn is_rising(avg_place: f64, prev_place: Option<f64>, d3_place: Option<f64>, cfg: &TierConfig) -> bool {
    let line = -cfg.rising_places + 1e-9; // "at least": a difference of exactly the line counts
    prev_place.is_some_and(|p| avg_place - p <= line) || d3_place.is_some_and(|d| d - avg_place <= line)
}

/// Patch order as the API sorts them (`patch_order` in tft-api): newest first by major, then minor number.
fn newer_first(a: &str, b: &str) -> Ordering {
    let parts = |p: &str| -> (f64, f64) {
        let mut it = p.split('.').map(|x| x.trim().parse::<f64>().unwrap_or(f64::NAN));
        (it.next().unwrap_or(f64::NAN), it.next().unwrap_or(f64::NAN))
    };
    let ((a1, a2), (b1, b2)) = (parts(a), parts(b));
    let first = b1 - a1;
    let d = if first != 0.0 && !first.is_nan() { first } else { b2 - a2 };
    d.partial_cmp(&0.0).unwrap_or(Ordering::Equal)
}

/// The patch before `patch` among the patches with stats, as the API's `prevPatch`.
fn previous_patch(db: &Connection, patch: &str) -> Result<Option<String>> {
    let mut patches: Vec<String> =
        db.prepare("SELECT DISTINCT patch FROM stat_meta")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    patches.sort_by(|a, b| newer_first(a, b));
    Ok(patches.iter().position(|p| p == patch).and_then(|i| patches.get(i + 1).cloned()))
}

/// Works out score, tier, prev_place and d3_place for one scope's comps, from the stat tables as they stand.
pub fn finish_scope(db: &Connection, patch: &str, scope: &str) -> Result<()> {
    let comps: Vec<CompSums> = db
        .prepare("SELECT comp_key, games, sum_place, wins, top4 FROM stat_comps WHERE patch = ? AND bucket = ? ORDER BY comp_key")?
        .query_map(params![patch, scope], |r| {
            Ok(CompSums { key: r.get(0)?, games: r.get(1)?, sum_place: r.get(2)?, wins: r.get(3)?, top4: r.get(4)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if comps.is_empty() {
        return Ok(());
    }
    let graded = grade(&comps, config());
    let (bucket, window) = scope.split_once('@').unwrap_or((scope, ""));
    let prev = previous_patch(db, patch)?;
    let d3_scope = if window.is_empty() { Some(tft_core::config::scope_key(bucket, "3d")) } else { None };
    let mut set = db.prepare_cached(
        "UPDATE stat_comps SET score = ?4, tier = ?5,
           prev_place = (SELECT sum_place * 1.0 / games FROM stat_comps WHERE patch = ?6 AND bucket = ?7 AND comp_key =
             COALESCE((SELECT prev_comp_id FROM comp_lineage WHERE patch = ?1 AND comp_id = ?3 AND prev_patch = ?6), ?3)),
           d3_place = (SELECT sum_place * 1.0 / games FROM stat_comps WHERE patch = ?1 AND bucket = ?8 AND comp_key = ?3)
         WHERE patch = ?1 AND bucket = ?2 AND comp_key = ?3",
    )?;
    for (c, (score, tier)) in comps.iter().zip(graded) {
        set.execute(params![patch, scope, c.key, score, tier, prev, bucket, d3_scope])?;
    }
    Ok(())
}

/// [`finish_scope`] for every scope with stats. Trends read other scopes, so this runs once all are written.
pub fn finish_all(db: &Connection) -> Result<()> {
    let scopes: Vec<(String, String)> = db
        .prepare("SELECT patch, bucket FROM stat_meta ORDER BY patch, bucket")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (patch, scope) in scopes {
        finish_scope(db, &patch, &scope)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TierConfig {
        config().clone()
    }

    fn comp(key: &str, games: i64, sum_place: i64, wins: i64, top4: i64) -> CompSums {
        CompSums { key: key.into(), games, sum_place, wins, top4 }
    }

    #[test]
    fn the_config_file_is_the_prd_formula() {
        let c = cfg();
        assert_eq!((c.weights.avg_place, c.weights.top4_rate, c.weights.win_rate), (0.55, 0.25, 0.2));
        assert_eq!(c.confidence_games, 400.0);
        let cuts: Vec<(&str, i64)> = c.tiers.iter().map(|t| (t.tier.as_str(), t.percent)).collect();
        assert_eq!(cuts, [("S", 8), ("A", 17), ("B", 30), ("C", 30), ("D", 15)]);
        assert_eq!(c.tiers.iter().map(|t| t.percent).sum::<i64>(), 100, "the shares cover every comp");
        assert_eq!(c.rising_places, 0.15);
    }

    #[test]
    fn z_scores_use_the_population_deviation() {
        // mean 4, deviations -2, 0, 2: population sd = sqrt(8/3)
        let z = z_scores(&[2.0, 4.0, 6.0]);
        let sd = (8.0f64 / 3.0).sqrt();
        assert!((z[0] + 2.0 / sd).abs() < 1e-12 && z[1].abs() < 1e-12 && (z[2] - 2.0 / sd).abs() < 1e-12, "{z:?}");
        assert_eq!(z_scores(&[4.5, 4.5]), [0.0, 0.0], "no spread: every z is 0");
        assert!(z_scores(&[]).is_empty());
    }

    #[test]
    fn confidence_is_one_half_at_400_games() {
        let c = cfg();
        assert_eq!(confidence(400, &c), 0.5);
        assert_eq!(confidence(0, &c), 0.0);
        assert!((confidence(1200, &c) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn scores_weigh_place_top4_and_wins_then_shrink_by_confidence() {
        let c = cfg();
        // Two comps, same games (400): z is +1 / -1 on each rate.
        // A: avg 3.5, top4 60%, win 20%. B: avg 4.5, top4 40%, win 10%.
        let comps = [comp("A", 400, 1400, 80, 240), comp("B", 400, 1800, 40, 160)];
        let g = grade(&comps, &c);
        // score A = 0.55 * 1 + 0.25 * 1 + 0.20 * 1 = 1.0, times confidence 0.5
        assert!((g[0].0 - 0.5).abs() < 1e-12, "{g:?}");
        assert!((g[1].0 + 0.5).abs() < 1e-12, "{g:?}");
        // Same rates, but B has 4x the games of A: only confidence changes.
        let comps = [comp("A", 400, 1400, 80, 240), comp("B", 1600, 7200, 160, 640)];
        let g = grade(&comps, &c);
        assert!((g[0].0 - 0.5).abs() < 1e-12 && (g[1].0 + 0.8).abs() < 1e-12, "{g:?}");
        // Place weighs most: C only places better, D only wins and top 4s more.
        let comps = [comp("C", 400, 1400, 40, 160), comp("D", 400, 1800, 80, 240)];
        let g = grade(&comps, &c);
        assert!(g[0].0 > g[1].0, "{g:?}");
        assert!((g[0].0 - 0.5 * (0.55 - 0.45)).abs() < 1e-12, "{g:?}");
    }

    /// 100 comps, best first by construction: avg place rises with the index, rates fall, games equal.
    fn ladder(n: usize) -> Vec<CompSums> {
        (0..n)
            .map(|i| {
                let g = 1000;
                comp(&format!("K{i:03}"), g, 3000 + 20 * i as i64, 200 - i as i64, 700 - 3 * i as i64)
            })
            .collect()
    }

    fn letters(n: usize) -> String {
        grade(&ladder(n), &cfg()).into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn tiers_take_their_share_of_the_ranking() {
        // 100 comps: 8 S, 17 A, 30 B, 30 C, 15 D, exactly on the cut-offs.
        let want = format!("{}{}{}{}{}", "S".repeat(8), "A".repeat(17), "B".repeat(30), "C".repeat(30), "D".repeat(15));
        assert_eq!(letters(100), want);
        // 10 comps: running totals 0.8, 2.5, 5.5, 8.5 round to 1, 3, 6, 9 (halves up).
        assert_eq!(letters(10), "SAABBBCCCD");
        // 12: 0.96, 3, 6.6, 10.2 -> 1, 3, 7, 10.
        assert_eq!(letters(12), "SAABBBBCCCDD");
        // Few comps sit in the middle: 1 -> B; 2 -> 0.16, 0.5 (rounds up), 1.1, 1.7 -> A, C.
        assert_eq!(letters(1), "B");
        assert_eq!(letters(2), "AC");
        assert_eq!(letters(0), "");
    }

    #[test]
    fn ties_rank_by_games_then_key() {
        // Identical rates: every z is 0, so every final is 0. More games ranks higher, then the key.
        let comps = [comp("B", 10, 45, 1, 5), comp("A", 10, 45, 1, 5), comp("C", 20, 90, 2, 10)];
        let g = grade(&comps, &cfg());
        assert!(g.iter().all(|(s, _)| *s == 0.0));
        let mut ranked: Vec<(&str, &str)> = comps.iter().zip(&g).map(|(c, (_, t))| (c.key.as_str(), t.as_str())).collect();
        ranked.sort_by_key(|(_, t)| t.to_string());
        // 3 comps: 0.24, 0.75, 1.65, 2.55 -> 0, 1, 2, 3: A, B, C.
        assert_eq!(ranked, [("C", "A"), ("A", "B"), ("B", "C")]);
    }

    #[test]
    fn rising_means_at_least_the_line_better() {
        let c = cfg();
        assert!(is_rising(4.35, Some(4.5), None, &c), "exactly 0.15 better than last patch");
        assert!(!is_rising(4.36, Some(4.5), None, &c));
        assert!(is_rising(4.5, None, Some(4.35), &c), "last 3 days 0.15 better than the whole patch");
        assert!(!is_rising(4.5, None, Some(4.4), &c));
        assert!(!is_rising(4.5, Some(4.2), Some(4.9), &c), "worse on both");
        assert!(is_rising(4.5, Some(4.9), Some(4.9), &c), "either one is enough");
        assert!(!is_rising(4.0, None, None, &c), "nothing to compare with");
    }

    #[test]
    fn patches_sort_newest_first_by_number() {
        let mut ps = vec!["16.9", "16.22", "15.24", "16.10"];
        ps.sort_by(|a, b| newer_first(a, b));
        assert_eq!(ps, ["16.22", "16.10", "16.9", "15.24"]);
    }
}
