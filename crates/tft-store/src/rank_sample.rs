//! Real ranks for a sample of lobbies (E7). Each stored match carries the rank of the player it was found
//! through (`matches.rank`), which stands in for the whole lobby. For a small, fixed share of matches the crawler
//! looks up all eight players' real ranks, so we can see how often that stand-in puts the lobby in the right
//! rank bucket. Tables: migration 1 in [`crate::schema::MIGRATIONS`].

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use tft_core::config::{BUCKETS, tier_rank};

/// Tier stored for a player with no ranked TFT entry. Counts as the lowest rank.
pub const UNRANKED: &str = "UNRANKED";

/// Whether a match is one of the sampled ones. Chosen by a hash of the match id (64-bit FNV-1a, a fixed
/// function that does not change between builds or runs), so a resumed crawl samples the same matches, and both
/// engines could agree if the TypeScript side ever needed to. `rate` is the share, 0 to 1.
pub fn is_sampled(match_id: &str, rate: f64) -> bool {
    if rate <= 0.0 {
        return false;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in match_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ((h % 1_000_000) as f64) < rate * 1_000_000.0
}

/// Marks a stored match for sampling. Doing it again (after a restart) changes nothing, even once it is done.
pub fn queue_sample(conn: &Connection, match_id: &str, platform: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO rank_sample_queue (match_id, platform) VALUES (?1, ?2)",
        params![match_id, platform],
    )?;
    Ok(())
}

/// Up to `limit` sampled matches on these servers that are not finished, leaving out the ones in `skip`, each
/// with the players whose rank has not been looked up yet: `(match_id, platform, puuids)`.
pub fn pending_samples(
    conn: &Connection,
    platforms: &[String],
    skip: &[String],
    limit: i64,
) -> Result<Vec<(String, String, Vec<String>)>> {
    let platforms = serde_json::to_string(platforms)?;
    let skip = serde_json::to_string(skip)?;
    let picked: Vec<(String, String)> = conn
        .prepare_cached(
            "SELECT match_id, platform FROM rank_sample_queue
             WHERE done = 0 AND platform IN (SELECT value FROM json_each(?1))
             AND match_id NOT IN (SELECT value FROM json_each(?2)) ORDER BY match_id LIMIT ?3",
        )?
        .query_map(params![platforms, skip, limit], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut missing = conn.prepare_cached(
        "SELECT p.puuid FROM participants p WHERE p.match_id = ?1
         AND NOT EXISTS (SELECT 1 FROM rank_samples s WHERE s.match_id = p.match_id AND s.puuid = p.puuid)
         ORDER BY p.id",
    )?;
    let mut out = Vec::with_capacity(picked.len());
    for (id, platform) in picked {
        let puuids = missing.query_map([&id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        out.push((id, platform, puuids));
    }
    Ok(out)
}

/// Stores one player's real rank in a sampled match.
pub fn save_rank(conn: &Connection, match_id: &str, puuid: &str, tier: &str, division: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO rank_samples (match_id, puuid, tier, division) VALUES (?1, ?2, ?3, ?4)",
        params![match_id, puuid, tier, division],
    )?;
    Ok(())
}

/// Marks a sampled match done if every player in it has a rank stored. Returns whether it is done.
pub fn complete_sample(conn: &Connection, match_id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE rank_sample_queue SET done = 1 WHERE match_id = ?1 AND NOT EXISTS (
           SELECT 1 FROM participants p WHERE p.match_id = ?1
           AND NOT EXISTS (SELECT 1 FROM rank_samples s WHERE s.match_id = p.match_id AND s.puuid = p.puuid))",
        [match_id],
    )?;
    Ok(n > 0)
}

/// The narrowest rank bucket a rank number falls in (`MASTER+` for Master and up, and so on).
pub fn bucket_of(rank: i64) -> &'static str {
    BUCKETS.iter().rev().find(|b| rank >= b.min).map_or(BUCKETS[0].key, |b| b.key)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BucketAccuracy {
    /// The bucket the stored rank puts these lobbies in.
    pub bucket: &'static str,
    pub sampled_matches: i64,
    pub in_right_bucket: i64,
    /// Where those lobbies really are: one count per bucket, in [`BUCKETS`] order.
    pub real: Vec<(&'static str, i64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RankSampleAccuracy {
    /// Sampled matches with every player's rank looked up. Unfinished ones are left out.
    pub sampled_matches: i64,
    /// Players looked up in those matches.
    pub players: i64,
    /// Share of those matches whose stored rank puts them in the right bucket. None when there are none yet.
    pub share_in_right_bucket: Option<f64>,
    /// One entry per bucket in [`BUCKETS`] order, by the bucket the stored rank gives.
    pub per_bucket: Vec<BucketAccuracy>,
}

/// How often a match's stored rank (`matches.rank`, the seed player's tier) puts the lobby in the right bucket.
///
/// The right bucket is the one implied by the lobby's real ranks, defined as the bucket of its *upper median*
/// real rank: sort the players' real ranks low to high and take the one at index `n / 2` (the 5th of 8). Put
/// plainly, it is the narrowest bucket that at least half the lobby has reached. Unranked players count as the
/// lowest rank. A lobby is "in the right bucket" when that real bucket equals the bucket of its stored rank.
pub fn rank_sample_accuracy(conn: &Connection) -> Result<RankSampleAccuracy> {
    let done: Vec<(String, i64)> = conn
        .prepare(
            "SELECT q.match_id, m.rank FROM rank_sample_queue q JOIN matches m ON m.match_id = q.match_id
             WHERE q.done = 1 ORDER BY q.match_id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut tiers = conn.prepare_cached("SELECT tier FROM rank_samples WHERE match_id = ?1")?;
    let index = |key: &str| BUCKETS.iter().position(|b| b.key == key).unwrap_or(0);
    let mut per_bucket: Vec<BucketAccuracy> = BUCKETS
        .iter()
        .map(|b| BucketAccuracy {
            bucket: b.key,
            sampled_matches: 0,
            in_right_bucket: 0,
            real: BUCKETS.iter().map(|r| (r.key, 0)).collect(),
        })
        .collect();
    let (mut matches, mut players, mut right) = (0, 0, 0);
    for (id, stored_rank) in done {
        let mut ranks: Vec<i64> = tiers
            .query_map([&id], |r| r.get::<_, Option<String>>(0))?
            .map(|t| t.map(|t| tier_rank(t.as_deref().unwrap_or(UNRANKED))))
            .collect::<rusqlite::Result<_>>()?;
        if ranks.is_empty() {
            continue;
        }
        ranks.sort_unstable();
        let real = bucket_of(ranks[ranks.len() / 2]);
        let estimated = bucket_of(stored_rank);
        let entry = &mut per_bucket[index(estimated)];
        entry.sampled_matches += 1;
        entry.real[index(real)].1 += 1;
        if real == estimated {
            entry.in_right_bucket += 1;
            right += 1;
        }
        matches += 1;
        players += ranks.len() as i64;
    }
    Ok(RankSampleAccuracy {
        sampled_matches: matches,
        players,
        share_in_right_bucket: (matches > 0).then(|| right as f64 / matches as f64),
        per_bucket,
    })
}

/// Whether a match is waiting for rank lookups. For tests and the health view.
pub fn sample_done(conn: &Connection, match_id: &str) -> Result<Option<bool>> {
    Ok(conn
        .query_row("SELECT done FROM rank_sample_queue WHERE match_id = ?1", [match_id], |r| r.get::<_, i64>(0))
        .optional()?
        .map(|d| d == 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA).unwrap();
        crate::apply_migrations(&conn).unwrap();
        crate::apply_migrations(&conn).unwrap(); // idempotent
        conn
    }

    /// A stored match with the given stored rank and one player per real tier, all already looked up.
    fn lobby(conn: &Connection, id: &str, stored_rank: i64, real: &[&str]) {
        conn.execute(
            "INSERT INTO matches (match_id, platform, patch, game_datetime, rank) VALUES (?1, 'na1', '99.3', 0, ?2)",
            params![id, stored_rank],
        )
        .unwrap();
        queue_sample(conn, id, "na1").unwrap();
        for (i, tier) in real.iter().enumerate() {
            let puuid = format!("{id}-{i}");
            conn.execute("INSERT INTO participants (match_id, puuid, placement) VALUES (?1, ?2, ?3)", params![id, puuid, i as i64 + 1])
                .unwrap();
            save_rank(conn, id, &puuid, tier, "I").unwrap();
        }
        assert!(complete_sample(conn, id).unwrap());
    }

    #[test]
    fn sampling_picks_about_the_configured_share_and_the_same_matches_every_time() {
        let ids: Vec<String> = (0..200_000).map(|i| format!("NA1_{}", 5_000_000_000u64 + i)).collect();
        for rate in [0.01, 0.05, 0.5] {
            let picked = ids.iter().filter(|id| is_sampled(id, rate)).count() as f64 / ids.len() as f64;
            assert!((picked - rate).abs() < rate * 0.1, "rate {rate}: picked {picked}");
        }
        let first: Vec<bool> = ids.iter().map(|id| is_sampled(id, 0.01)).collect();
        let again: Vec<bool> = ids.iter().map(|id| is_sampled(id, 0.01)).collect();
        assert_eq!(first, again);
        // A fixed function: these ids are sampled at 1% in every build.
        let known: Vec<&String> = ids.iter().filter(|id| is_sampled(id, 0.01)).take(3).collect();
        assert_eq!(known, ["NA1_5000000006", "NA1_5000000029", "NA1_5000000068"]);
        assert!(!ids.iter().any(|id| is_sampled(id, 0.0)), "rate 0 samples nothing");
        assert!(ids.iter().all(|id| is_sampled(id, 1.0)), "rate 1 samples everything");
        // A higher rate keeps every match a lower rate picked.
        assert!(ids.iter().filter(|id| is_sampled(id, 0.01)).all(|id| is_sampled(id, 0.05)));
    }

    #[test]
    fn buckets_are_the_narrowest_that_fits() {
        assert_eq!(bucket_of(0), "ALL");
        assert_eq!(bucket_of(3), "ALL");
        assert_eq!(bucket_of(4), "PLATINUM+");
        assert_eq!(bucket_of(5), "PLATINUM+");
        assert_eq!(bucket_of(6), "DIAMOND+");
        assert_eq!(bucket_of(7), "MASTER+");
        assert_eq!(bucket_of(9), "MASTER+");
    }

    #[test]
    fn pending_lists_only_players_not_looked_up_and_done_needs_all_of_them() {
        let conn = db();
        conn.execute("INSERT INTO matches (match_id, platform, patch, game_datetime, rank) VALUES ('M', 'na1', '99.3', 0, 6)", [])
            .unwrap();
        for i in 0..8 {
            conn.execute("INSERT INTO participants (match_id, puuid, placement) VALUES ('M', ?1, ?2)", params![format!("p{i}"), i as i64 + 1])
                .unwrap();
        }
        queue_sample(&conn, "M", "na1").unwrap();
        queue_sample(&conn, "M", "na1").unwrap();
        assert_eq!(pending_samples(&conn, &["euw1".into()], &[], 10).unwrap(), vec![]);
        let p = pending_samples(&conn, &["na1".into()], &[], 10).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].2.len(), 8);
        assert!(pending_samples(&conn, &["na1".into()], &["M".into()], 10).unwrap().is_empty(), "skipped");
        for i in 0..7 {
            save_rank(&conn, "M", &format!("p{i}"), "DIAMOND", "II").unwrap();
        }
        assert!(!complete_sample(&conn, "M").unwrap(), "one player is still missing");
        assert_eq!(pending_samples(&conn, &["na1".into()], &[], 10).unwrap()[0].2, ["p7"]);
        save_rank(&conn, "M", "p7", UNRANKED, "").unwrap();
        assert!(complete_sample(&conn, "M").unwrap());
        assert_eq!(sample_done(&conn, "M").unwrap(), Some(true));
        assert!(pending_samples(&conn, &["na1".into()], &[], 10).unwrap().is_empty());
        queue_sample(&conn, "M", "na1").unwrap();
        assert_eq!(sample_done(&conn, "M").unwrap(), Some(true), "queueing a finished match again leaves it done");
    }

    #[test]
    fn accuracy_on_a_hand_made_set_of_lobbies() {
        let conn = db();
        assert_eq!(rank_sample_accuracy(&conn).unwrap().share_in_right_bucket, None);
        const M: &str = "MASTER";
        const D: &str = "DIAMOND";
        const E: &str = "EMERALD";
        const P: &str = "PLATINUM";
        const G: &str = "GOLD";
        // Stored Master, really Master+: 4 of 8 are Master or higher, so the upper median (5th of 8) is Master.
        lobby(&conn, "A", 7, &[G, P, D, D, M, M, "GRANDMASTER", "CHALLENGER"]);
        // Stored Master, really Diamond+: only 3 of 8 reach Master.
        lobby(&conn, "B", 7, &[G, P, D, D, D, M, M, M]);
        // Stored Diamond, really Diamond+.
        lobby(&conn, "C", 6, &[E, E, D, D, D, D, M, M]);
        // Stored Platinum, really Platinum+ (Emerald is inside Platinum+).
        lobby(&conn, "D", 4, &[G, G, P, E, E, E, D, D]);
        // Stored Emerald (Platinum+), really All: unranked players count as the lowest rank.
        lobby(&conn, "E", 5, &[UNRANKED, UNRANKED, UNRANKED, UNRANKED, G, G, P, P]);
        // Not finished: left out.
        conn.execute("INSERT INTO matches (match_id, platform, patch, game_datetime, rank) VALUES ('F', 'na1', '99.3', 0, 9)", [])
            .unwrap();
        conn.execute("INSERT INTO participants (match_id, puuid, placement) VALUES ('F', 'f0', 1)", []).unwrap();
        queue_sample(&conn, "F", "na1").unwrap();

        let a = rank_sample_accuracy(&conn).unwrap();
        assert_eq!(a.sampled_matches, 5);
        assert_eq!(a.players, 40);
        assert_eq!(a.share_in_right_bucket, Some(3.0 / 5.0));
        let by = |k: &str| a.per_bucket.iter().find(|b| b.bucket == k).unwrap().clone();
        assert_eq!(a.per_bucket.iter().map(|b| b.bucket).collect::<Vec<_>>(), ["ALL", "PLATINUM+", "DIAMOND+", "MASTER+"]);
        let master = by("MASTER+");
        assert_eq!((master.sampled_matches, master.in_right_bucket), (2, 1));
        assert_eq!(master.real, [("ALL", 0), ("PLATINUM+", 0), ("DIAMOND+", 1), ("MASTER+", 1)]);
        assert_eq!((by("DIAMOND+").sampled_matches, by("DIAMOND+").in_right_bucket), (1, 1));
        let plat = by("PLATINUM+");
        assert_eq!((plat.sampled_matches, plat.in_right_bucket), (2, 1));
        assert_eq!(plat.real, [("ALL", 1), ("PLATINUM+", 1), ("DIAMOND+", 0), ("MASTER+", 0)]);
        assert_eq!(by("ALL").sampled_matches, 0);
    }
}
