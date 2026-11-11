//! PRD 2 test plan for the clusters comp method (C1, C2, C13, C14): hand-built boards for six known comps,
//! determinism, recount, separation, variants, lineage and your overrides, and incremental builds equal to full
//! ones. The speed check on a synthetic 8 million boards is ignored by default (see the last test).

use super::*;
use crate::cluster::{Boards, BoardsBuilder, Method, cluster, overlap, parse_list, set_comp_method};
use crate::incremental_tests::{DAY, Game, assert_same_stats, fresh_db, store};
use std::collections::HashMap;
use tft_core::patches::{Calendar, PatchStart};

/// One board: its label (which comp it was built as), placement, level and units (id, cost, star, items).
#[derive(Clone)]
struct Board {
    label: String,
    place: i64,
    level: i64,
    units: Vec<(String, i64, i64, i64)>,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const RARITY: [i64; 5] = [0, 1, 2, 4, 6];

/// A comp as a template: (unit, cost, star, items).
type Template = Vec<(&'static str, i64, i64, i64)>;

fn six_comps() -> Vec<(&'static str, Template, i64)> {
    vec![
        ("A", vec![("A1", 1, 2, 0), ("A2", 1, 2, 0), ("A3", 2, 2, 0), ("A4", 2, 2, 0), ("A5", 3, 2, 0), ("A6", 3, 2, 0), ("A7", 4, 2, 3), ("A8", 4, 2, 1)], 8),
        ("B", vec![("B1", 1, 2, 0), ("B2", 2, 2, 0), ("B3", 2, 2, 0), ("B4", 3, 2, 0), ("B5", 3, 2, 0), ("B6", 4, 2, 1), ("B7", 5, 2, 3), ("B8", 5, 1, 0)], 9),
        ("C", vec![("C1", 1, 2, 0), ("C2", 1, 2, 0), ("C3", 2, 2, 0), ("C4", 3, 2, 0), ("C5", 3, 2, 3), ("C6", 4, 2, 1), ("C7", 4, 1, 0), ("C8", 2, 2, 0)], 8),
        // Two carries: either can end up with the items. The baseline rule labels these two ways.
        ("D", vec![("D1", 1, 2, 0), ("D2", 2, 2, 0), ("D3", 2, 2, 0), ("D4", 3, 2, 0), ("D5", 4, 2, 0), ("D6", 4, 2, 3), ("D7", 5, 2, 0), ("D8", 3, 2, 0)], 8),
        // Reroll: the 1-cost carry and a 2-cost at three stars, level 7.
        ("E", vec![("E1", 1, 3, 3), ("E2", 2, 3, 1), ("E3", 1, 2, 0), ("E4", 2, 2, 0), ("E5", 2, 2, 0), ("E6", 3, 2, 0), ("E7", 4, 2, 0), ("E8", 5, 1, 0)], 7),
        // Its fast-9 cousin: the same eight units plus a second 5-cost, the 5-cost carrying, nothing at three stars.
        ("F", vec![("E1", 1, 2, 0), ("E2", 2, 2, 0), ("E3", 1, 2, 0), ("E4", 2, 2, 0), ("E5", 2, 2, 0), ("E6", 3, 2, 0), ("E7", 4, 2, 1), ("E8", 5, 2, 3), ("F9", 5, 1, 0)], 9),
    ]
}

/// `per` boards of each comp: sometimes a unit left out, a 1-cost filler added, or the carry one item short
/// (a board with no three-item unit). Comp D's items go on D7 instead of D6 in half the boards.
fn fixture(seed: u64, per: usize, comps: &[(&'static str, Template, i64)]) -> Vec<Board> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    for (label, units, level) in comps {
        for i in 0..per {
            let mut us: Vec<(String, i64, i64, i64)> = units.iter().map(|(u, c, s, n)| (format!("TFT99_{u}"), *c, *s, *n)).collect();
            if *label == "D" && i % 2 == 1 {
                us[5].3 = 0;
                us[6].3 = 3;
            }
            if rng.below(10) < 3 {
                let k = rng.below(us.len() as u64) as usize;
                if us[k].3 < 3 {
                    us.remove(k);
                }
            }
            if rng.below(10) < 4 {
                us.push((format!("TFT99_Z{}", rng.below(4)), 1, 1, 0));
            }
            if rng.below(10) == 0 {
                for u in &mut us {
                    if u.3 == 3 {
                        u.3 = 2;
                    }
                }
            }
            out.push(Board { label: label.to_string(), place: 1 + (i % 8) as i64, level: *level, units: us });
        }
    }
    out
}

/// Stores boards eight to a match (rank 8), in patch `patch` at time `at`. Each board's stored (baseline)
/// label is "label|first unit with three items".
fn put(conn: &Connection, patch: &str, at: i64, first_match: usize, boards: &[Board]) -> Vec<i64> {
    let mut ids = Vec::new();
    for (i, b) in boards.iter().enumerate() {
        let m = format!("{patch}-M{}", first_match + i / 8);
        conn.execute("INSERT OR IGNORE INTO matches VALUES (?, 'na1', ?, 99, ?, 2000, 8, NULL)", params![m, patch, at]).unwrap();
        let carry = b.units.iter().find(|u| u.3 >= 3).map_or("none", |u| u.0.as_str());
        conn.execute(
            "INSERT INTO participants (match_id, puuid, placement, level, comp_key) VALUES (?, ?, ?, ?, ?)",
            params![m, format!("{m}-{i}"), b.place, b.level, format!("T_{}|{carry}", b.label)],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        for (u, cost, star, items) in &b.units {
            conn.execute("INSERT INTO units VALUES (?, ?, ?, ?, NULL, ?)", params![id, u, star, items, RARITY[*cost as usize - 1]])
                .unwrap();
            for k in 0..*items {
                conn.execute("INSERT INTO unit_items VALUES (?, ?, ?)", params![id, u, format!("I{k}")]).unwrap();
            }
        }
        conn.execute("INSERT INTO traits VALUES (?, ?, 6, 2, 2)", params![id, format!("T_{}", b.label)]).unwrap();
        ids.push(id);
    }
    ids
}

/// Which listed comp each board of a patch (whole patch, all ranks) belongs to, worked out the way the build
/// does it: the same boards through the same steps.
fn members(conn: &Connection, patch: &str) -> HashMap<i64, String> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
         DELETE FROM p;",
    )
    .unwrap();
    conn.execute(
        &format!(
            "INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
             JOIN matches m ON m.match_id = pa.match_id WHERE m.patch = ? AND m.rank >= ? AND {}",
            ranked_only("m")
        ),
        params![patch, BUCKETS[0].min],
    )
    .unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM p", [], |r| r.get(0)).unwrap();
    let min_games = COMP_MIN_GAMES.max(((n as f64) * COMP_MIN_SHARE).ceil() as i64);
    cluster::label(conn, min_games, COMP_LIMIT).unwrap();
    overrides::apply_to_boards(conn).unwrap();
    let out = conn
        .prepare(
            "SELECT id, comp_key FROM p WHERE comp_key IN (SELECT comp_key FROM stat_comps WHERE patch = ? AND bucket = 'ALL')",
        )
        .unwrap()
        .query_map([patch], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    conn.execute_batch("DROP TABLE p").unwrap();
    out
}

/// The comp most of a label's boards went to, and that share.
fn majority(ids: &[i64], boards: &[Board], label: &str, mem: &HashMap<i64, String>) -> (String, f64) {
    let mut count: HashMap<&str, usize> = HashMap::new();
    let mine: Vec<i64> = ids.iter().zip(boards).filter(|(_, b)| b.label == label).map(|(i, _)| *i).collect();
    for id in &mine {
        if let Some(c) = mem.get(id) {
            *count.entry(c.as_str()).or_default() += 1;
        }
    }
    let (c, n) = count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).unwrap_or(("", 0));
    (c.to_string(), n as f64 / mine.len() as f64)
}

fn q(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn clusters_db(name: &str) -> (std::path::PathBuf, Connection) {
    let (dir, conn) = fresh_db(name);
    set_comp_method(&conn, Method::Clusters).unwrap();
    (dir, conn)
}

#[test]
fn six_known_comps_cluster_as_labelled() {
    let (dir, mut conn) = clusters_db("six");
    let mut boards = fixture(11, 120, &six_comps());
    // Two odd A boards: a 5-cost nobody else plays holds the items instead of A7. Their bucket is too small to
    // take part in merging; they join A by their units.
    for _ in 0..2 {
        let mut odd = boards[0].clone();
        odd.units.retain(|u| u.0 != "TFT99_A7");
        odd.units.push(("TFT99_Q5".into(), 5, 2, 3));
        boards.push(odd);
    }
    let ids = put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let mem = members(&conn, "91.1");

    let mut seen: Vec<String> = Vec::new();
    for label in ["A", "B", "C", "D", "E", "F"] {
        let (comp, share) = majority(&ids, &boards, label, &mem);
        assert!(share >= 0.9, "comp {label}: only {share:.2} of its boards landed together");
        assert!(!seen.contains(&comp), "comp {label} landed in the same comp as another label");
        seen.push(comp.clone());
        // Purity: the comp holds (almost) only this label's boards.
        let theirs = ids.iter().zip(&boards).filter(|(i, _)| mem.get(i) == Some(&comp)).collect::<Vec<_>>();
        let own = theirs.iter().filter(|(_, b)| b.label == label).count();
        assert!(own as f64 / theirs.len() as f64 >= 0.9, "comp {label} is mixed with other boards");
    }
    // The reroll comp and its fast-9 cousin are apart, and the reroll core says its carry is three-starred.
    let core = |id: &str| -> Vec<String> {
        parse_list(&conn.query_row("SELECT core FROM stat_comps WHERE bucket = 'ALL' AND comp_key = ?", [id], |r| r.get::<_, String>(0)).unwrap())
    };
    let (e, f) = (majority(&ids, &boards, "E", &mem).0, majority(&ids, &boards, "F", &mem).0);
    assert!(core(&e).contains(&"TFT99_E1:3".to_string()), "{:?}", core(&e));
    assert!(core(&f).contains(&"TFT99_E1".to_string()), "{:?}", core(&f));
    // Boards with no three-item unit are placed by their units alone.
    for (id, b) in ids.iter().zip(&boards) {
        if b.units.iter().all(|u| u.3 < 3) {
            assert_eq!(mem.get(id), Some(&majority(&ids, &boards, &b.label, &mem).0), "a board of {} with no carry", b.label);
        }
    }
    let a = majority(&ids, &boards, "A", &mem).0;
    for id in &ids[ids.len() - 2..] {
        assert_eq!(mem.get(id), Some(&a), "an odd A board did not join A");
    }
    // Coverage and separation, as the side-by-side report measures them.
    let r = report::comps_report(&mut conn, Some("91.1"), None).unwrap();
    assert!(r.clusters.coverage() >= 0.9, "coverage {}", r.clusters.coverage());
    assert!(r.clusters.max_overlap.as_ref().is_some_and(|m| m.0 <= SEPARATION_LIMIT), "{:?}", r.clusters.max_overlap.as_ref().map(|m| m.0));
    assert_eq!(r.clusters.comps.len(), 6);
    let text = report::format_report(&r, 30);
    assert!(text.contains("Coverage (top 4 boards in a listed comp)"), "{text}");
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

const SEPARATION_LIMIT: f64 = crate::cluster::SEPARATION;

#[test]
fn two_rebuilds_and_any_board_order_give_the_same_comps() {
    let boards = fixture(5, 100, &six_comps());
    let (dir, mut a) = clusters_db("det-a");
    put(&a, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut a, 2, &mut |_| {}).unwrap();
    let listing = |c: &Connection| -> Vec<(String, String, i64, i64, String)> {
        c.prepare("SELECT bucket, comp_key, games, sum_place, core FROM stat_comps ORDER BY bucket, games DESC, comp_key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let first = listing(&a);
    assert!(first.len() >= 6 * 8, "six comps in each of the 8 scopes: {}", first.len());
    build_stats_with(&mut a, 2, &mut |_| {}).unwrap();
    assert_eq!(listing(&a), first, "a second rebuild changed ids or order");

    // The same boards stored in reverse order (other participant ids, other lobbies): the same tables.
    let (dir_b, mut b) = clusters_db("det-b");
    let reversed: Vec<Board> = boards.iter().rev().cloned().collect();
    put(&b, "91.1", now_ms() - 3_600_000, 0, &reversed);
    build_stats_with(&mut b, 2, &mut |_| {}).unwrap();
    drop(b);
    assert_same_stats(&a, &dir_b.join("t.db"));
    drop(a);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&dir_b).unwrap();
}

#[test]
fn every_comp_stat_is_a_recount_of_its_boards() {
    let (dir, mut conn) = clusters_db("recount");
    let mut boards = fixture(9, 90, &six_comps());
    // A 4-cost flex unit on 60% of A's boards: flex, not core, and a variant of its own.
    for (i, b) in boards.iter_mut().filter(|b| b.label == "A").enumerate() {
        if i % 5 < 3 {
            b.units.push(("TFT99_AF".into(), 4, 2, 0));
        }
    }
    put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let mem = members(&conn, "91.1");
    conn.execute_batch("CREATE TEMP TABLE mem (id INTEGER PRIMARY KEY, comp_key TEXT NOT NULL)").unwrap();
    for (id, c) in &mem {
        conn.execute("INSERT INTO mem VALUES (?, ?)", params![id, c]).unwrap();
    }
    let differ = |sql: &str| q(&conn, &format!("SELECT COUNT(*) FROM ({sql})"));
    let comps = "SELECT comp_key, games, sum_place, wins, top4, sum_level FROM stat_comps WHERE bucket = 'ALL'";
    let recount = "SELECT m.comp_key, COUNT(*), SUM(pa.placement), SUM(pa.placement = 1), SUM(pa.placement <= 4),
                   SUM(COALESCE(pa.level, 0)) FROM mem m JOIN participants pa ON pa.id = m.id GROUP BY m.comp_key";
    assert_eq!(differ(&format!("{comps} EXCEPT {recount}")), 0);
    assert_eq!(differ(&format!("{recount} EXCEPT {comps}")), 0);
    let units = "SELECT comp_key, character_id, games, sum_star, sum_items, full_items, sum_place FROM stat_comp_units WHERE bucket = 'ALL'";
    let unit_recount = "SELECT comp_key, character_id, COUNT(*), SUM(star), SUM(n_items), SUM(n_items = 3), SUM(placement)
        FROM (SELECT m.comp_key, u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, pa.placement
              FROM mem m JOIN participants pa ON pa.id = m.id JOIN units u ON u.participant_id = m.id GROUP BY m.id, u.character_id)
        GROUP BY comp_key, character_id";
    assert_eq!(differ(&format!("{units} EXCEPT {unit_recount}")), 0);
    assert_eq!(differ(&format!("{unit_recount} EXCEPT {units}")), 0);
    // Core and flex agree with the unit counts: core on 70% or more of the boards, flex on 20% to 70%.
    let mut st = conn.prepare("SELECT comp_key, games, core, flex FROM stat_comps WHERE bucket = 'ALL'").unwrap();
    let rows: Vec<(String, i64, String, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 6);
    for (key, games, core, flex) in rows {
        let share = |u: &str| -> f64 {
            conn.query_row("SELECT games FROM stat_comp_units WHERE bucket = 'ALL' AND comp_key = ? AND character_id = ?", [&key, u], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0) as f64
                / games as f64
        };
        let core = parse_list(&core);
        let flex = parse_list(&flex);
        assert!(core.len() >= cluster::MIN_CORE);
        for c in &core {
            assert!(share(c.trim_end_matches(":3")) >= 0.7, "{key}: core {c}");
        }
        for f in &flex {
            let s = share(f);
            assert!((0.2..0.7).contains(&s), "{key}: flex {f} at {s}");
        }
        assert_eq!(cluster::comp_id(&core), key, "the id is the hash of the core");
        if share("TFT99_A1") > 0.7 {
            assert!(flex.contains(&"TFT99_AF".to_string()) && !core.contains(&"TFT99_AF".to_string()), "{core:?} {flex:?}");
            let with_af: i64 = conn
                .query_row("SELECT COUNT(*) FROM stat_comp_variants WHERE bucket = 'ALL' AND comp_key = ? AND units = '[\"TFT99_AF\"]'", [&key], |r| r.get(0))
                .unwrap();
            assert_eq!(with_af, 1, "A runs a variant with its 4-cost flex unit");
        }
    }
    drop(st);
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The synthetic lobbies of the incremental tests: 30 archetypes over 60 units, a random extra unit, and two
/// carries per archetype.
fn lobbies(name: &str, method: Method) -> (std::path::PathBuf, Connection, Calendar, Vec<Game>, Vec<Game>) {
    let now = now_ms();
    let cal = Calendar(vec![PatchStart { patch: "90.1".into(), since: 0 }, PatchStart { patch: "90.2".into(), since: now - 8 * DAY }]);
    let ranks = [0, 3, 4, 5, 6, 7, 9];
    let mut first: Vec<Game> = (0..120).map(|i| (format!("A{i}"), ranks[i % 7], now - 20 * DAY + i as i64 * 60_000)).collect();
    first.extend((0..240).map(|i| (format!("B{i}"), ranks[i % 7], now - 7 * DAY + i as i64 * 2_000_000)));
    let more: Vec<Game> = (0..200).map(|i| (format!("C{i}"), 4 + (i % 2) as i64, now - 6 * DAY + i as i64 * 2_500_000)).collect();
    let (dir, conn) = fresh_db(name);
    set_comp_method(&conn, method).unwrap();
    tft_store::save_calendar(&conn, &cal).unwrap();
    (dir, conn, cal, first, more)
}

#[test]
fn no_two_listed_comps_share_more_than_three_quarters_of_their_core() {
    let (dir, mut conn, cal, first, more) = lobbies("separation", Method::Clusters);
    store(&mut conn, &cal, 1, &first);
    store(&mut conn, &cal, 2, &more);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let rows: Vec<(String, String, String)> = conn
        .prepare("SELECT patch || '/' || bucket, comp_key, core FROM stat_comps")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(rows.len() > 40, "{} comps", rows.len());
    let mut worst: f64 = 0.0;
    for (i, a) in rows.iter().enumerate() {
        for b in &rows[i + 1..] {
            if a.0 == b.0 {
                let o = overlap(&parse_list(&a.2), &parse_list(&b.2));
                assert!(o <= SEPARATION_LIMIT, "{} {} and {} overlap {o}", a.0, a.1, b.1);
                worst = worst.max(o);
            }
        }
    }
    assert!(worst > 0.3, "the data should have comps that share units ({worst})");
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn incremental_clusters_build_equals_a_full_rebuild() {
    let (dir, mut conn, cal, first, more) = lobbies("inc", Method::Clusters);
    store(&mut conn, &cal, 1, &first);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    store(&mut conn, &cal, 2, &more);
    let mut lines = Vec::new();
    let touched = build_stats_incremental_with(&mut conn, 2, &mut |s| lines.push(s.to_string())).unwrap();
    assert_eq!(touched, ["90.2/ALL", "90.2/ALL@3d", "90.2/PLATINUM+", "90.2/PLATINUM+@3d"]);
    assert!(lines.iter().any(|l| l.contains("added")), "{lines:?}");

    let (fresh_dir, mut fresh, ..) = lobbies("inc-fresh", Method::Clusters);
    store(&mut fresh, &cal, 1, &first);
    store(&mut fresh, &cal, 2, &more);
    build_stats_with(&mut fresh, 2, &mut |_| {}).unwrap();
    drop(fresh);
    let compared = assert_same_stats(&conn, &fresh_dir.join("t.db"));
    assert!(compared > 1000, "only {compared} rows compared");
    let variants = q(&conn, "SELECT COUNT(*) FROM stat_comp_variants");
    assert!(variants > 0, "the lobbies should make variants");
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&fresh_dir).unwrap();
}

/// Boards of one 8-unit comp whose three items go on a different unit per group: (index of the carry, boards).
fn carry_groups(prefix: &str, groups: &[(usize, i64)]) -> Vec<Board> {
    let units: Template = vec![("1", 1, 2, 0), ("2", 1, 2, 0), ("3", 2, 2, 0), ("4", 2, 2, 0), ("5", 3, 2, 0), ("6", 3, 2, 0), ("7", 4, 2, 0), ("8", 5, 2, 0)];
    let mut boards = Vec::new();
    for (carry, n) in groups {
        for i in 0..*n {
            let mut us: Vec<(String, i64, i64, i64)> =
                units.iter().map(|(u, c, s, k)| (format!("TFT99_{prefix}{u}"), *c, *s, *k)).collect();
            us[*carry].3 = 3;
            boards.push(Board { label: format!("{prefix}{carry}"), place: 1 + i % 8, level: 8, units: us });
        }
    }
    boards
}

#[test]
fn variants_need_fifty_games_and_at_most_four_are_listed() {
    // V: six ways to place the items, 70, 65, 60, 55 and 52 boards (the fifth is cut by the limit of four) and
    // 49. W: 60, 55 and 49 boards (under 50 games: not a variant).
    let mut boards = carry_groups("V", &[(6, 70), (7, 65), (5, 60), (4, 55), (3, 52), (2, 49)]);
    boards.extend(carry_groups("W", &[(6, 60), (7, 55), (5, 49)]));
    let (dir, mut conn) = clusters_db("variants");
    put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL'"), 2, "one comp each, whoever holds the items");
    let variants = |unit: &str| -> Vec<(String, String, i64, i64, i64, i64)> {
        conn.prepare(
            "SELECT units, carries, games, sum_place, wins, top4 FROM stat_comp_variants WHERE bucket = 'ALL'
             AND comp_key = (SELECT comp_key FROM stat_comp_units WHERE bucket = 'ALL' AND character_id = ?) ORDER BY games DESC",
        )
        .unwrap()
        .query_map([unit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    let vs = variants("TFT99_V1");
    assert_eq!(vs.iter().map(|v| v.2).collect::<Vec<_>>(), [70, 65, 60, 55]);
    assert_eq!(variants("TFT99_W1").iter().map(|v| v.2).collect::<Vec<_>>(), [60, 55]);
    let carries: Vec<Vec<String>> = vs.iter().map(|v| parse_list(&v.1)).collect();
    assert_eq!(carries, [["TFT99_V7"], ["TFT99_V8"], ["TFT99_V6"], ["TFT99_V5"]]);
    for v in &vs {
        let n = v.2;
        // Placements cycle 1..8 within each group.
        let places: Vec<i64> = (0..n).map(|i| 1 + i % 8).collect();
        assert_eq!(v.3, places.iter().sum::<i64>());
        assert_eq!(v.4, places.iter().filter(|p| **p == 1).count() as i64);
        assert_eq!(v.5, places.iter().filter(|p| **p <= 4).count() as i64);
        assert_eq!(v.0, "[]", "no 4- or 5-cost flex units here");
    }
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 3 compares weighted signatures: comps that share a bench of cheap units but play different expensive
/// units stay apart. Counting units alone, 6 shared of 10 would merge them.
#[test]
fn a_shared_cheap_bench_does_not_merge_comps_with_different_carries() {
    let bench = [("K1", 1), ("K2", 1), ("K3", 1), ("K4", 1), ("K5", 1), ("K6", 1)];
    let comp = |a: &'static str, b: &'static str| -> Template {
        let mut t: Template = bench.iter().map(|(u, c)| (*u, *c, 2, 0)).collect();
        t.push((a, 5, 2, 3));
        t.push((b, 5, 1, 0));
        t
    };
    let boards = fixture(8, 100, &[("K", comp("X1", "X2"), 8), ("L", comp("Y1", "Y2"), 8)]);
    let (dir, mut conn) = clusters_db("bench");
    let ids = put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let mem = members(&conn, "91.1");
    let (k, l) = (majority(&ids, &boards, "K", &mem), majority(&ids, &boards, "L", &mem));
    assert!(k.1 >= 0.9 && l.1 >= 0.9, "{k:?} {l:?}");
    assert_ne!(k.0, l.0, "the two comps were merged");
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Separation: two groups with the same core that step 3 leaves apart (the items sit on cheap units in one and on
/// the 5-costs in the other, so their signatures differ) are folded into one comp; the item split shows up as
/// its variants instead.
#[test]
fn comps_with_the_same_core_are_folded_together() {
    let units: Template = vec![("S1", 1, 2, 0), ("S2", 1, 2, 0), ("S3", 1, 2, 0), ("S4", 1, 2, 0), ("S5", 2, 2, 0), ("S6", 2, 2, 0), ("S7", 4, 2, 0), ("S8", 4, 2, 0)];
    let mut cheap = units.clone();
    for u in &mut cheap[..3] {
        u.3 = 3;
    }
    let mut costly = units.clone();
    for u in &mut costly[6..] {
        u.3 = 3;
    }
    let mut boards = fixture(2, 90, &[("cheap", cheap, 8)]);
    boards.extend(fixture(3, 80, &[("costly", costly, 8)]));
    // No fillers or short carries, so the two groups' signatures are known: about 0.52 alike, under MERGE_SIM,
    // and their heaviest units differ (S1 and S2 against S7 and S8), so they start in different buckets.
    for b in &mut boards {
        b.units.retain(|u| !u.0.contains("_Z"));
        for u in &mut b.units {
            u.3 = if b.label == "cheap" && ["TFT99_S1", "TFT99_S2", "TFT99_S3"].contains(&u.0.as_str())
                || b.label == "costly" && ["TFT99_S7", "TFT99_S8"].contains(&u.0.as_str())
            {
                3
            } else {
                0
            };
        }
    }
    let (dir, mut conn) = clusters_db("fold");
    put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL'"), 1);
    assert_eq!(q(&conn, "SELECT games FROM stat_comps WHERE bucket = 'ALL'"), 170);
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn favourites_renames_and_the_pin_follow_a_comp_into_the_next_patch() {
    let (dir, mut conn) = clusters_db("lineage");
    let old = fixture(3, 100, &six_comps());
    let old_ids = put(&conn, "91.1", now_ms() - 10 * DAY, 0, &old);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let mem = members(&conn, "91.1");
    let (a_old, b_old) = (majority(&old_ids, &old, "A", &mem).0, majority(&old_ids, &old, "B", &mem).0);
    routes_like_mark(&conn, &a_old);
    routes_like_mark(&conn, &b_old);
    conn.execute("INSERT INTO kv VALUES ('pinned', ?)", [format!("\"{a_old}\"")]).unwrap();
    overrides::rename(&conn, &a_old, "My A").unwrap();

    // Next patch: A swaps one core unit (a new id), B is a different comp altogether, the rest are unchanged.
    let mut comps = six_comps();
    comps[0].1[5].0 = "A6x";
    comps[1].1 = vec![("G1", 1, 2, 0), ("G2", 2, 2, 0), ("G3", 2, 2, 0), ("G4", 3, 2, 0), ("G5", 3, 2, 0), ("G6", 4, 2, 1), ("G7", 5, 2, 3), ("G8", 5, 1, 0)];
    let new = fixture(4, 100, &comps);
    let new_ids = put(&conn, "91.2", now_ms() - 3_600_000, 1000, &new);
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    let mem = members(&conn, "91.2");
    let (a_new, b_new, c_new) =
        (majority(&new_ids, &new, "A", &mem).0, majority(&new_ids, &new, "B", &mem).0, majority(&new_ids, &new, "C", &mem).0);
    assert_ne!(a_new, a_old, "A's core changed, so its id did");
    let link: (String, f64) = conn
        .query_row("SELECT prev_comp_id, overlap FROM comp_lineage WHERE patch = '91.2' AND comp_id = ?", [&a_new], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(link.0, a_old);
    assert!(link.1 > 0.6 && link.1 < 1.0, "{}", link.1);
    assert_eq!(q(&conn, &format!("SELECT fav FROM comp_marks WHERE comp_key = '{a_new}'")), 1, "the favourite carried over");
    assert_eq!(overrides::name_of(&conn, &a_new).unwrap().as_deref(), Some("My A"), "the rename carried over");
    // The trend (PRD 2 C4) follows the link: last patch's place is the old comp's.
    let place = |sql: &str, k: &str| -> f64 { conn.query_row(sql, [k], |r| r.get(0)).unwrap() };
    assert_eq!(
        place("SELECT prev_place FROM stat_comps WHERE patch = '91.2' AND bucket = 'ALL' AND comp_key = ?", &a_new),
        place("SELECT sum_place * 1.0 / games FROM stat_comps WHERE patch = '91.1' AND bucket = 'ALL' AND comp_key = ?", &a_old)
    );
    let pinned: String = conn.query_row("SELECT value FROM kv WHERE key = 'pinned'", [], |r| r.get(0)).unwrap();
    assert_eq!(pinned, format!("\"{a_new}\""));
    assert_eq!(q(&conn, &format!("SELECT COUNT(*) FROM comp_lineage WHERE comp_id = '{b_new}'")), 0, "B is new");
    assert_eq!(q(&conn, &format!("SELECT COUNT(*) FROM comp_marks WHERE comp_key = '{b_new}'")), 0);
    // An unchanged comp keeps its id.
    let c_old = majority(&old_ids, &old, "C", &members(&conn, "91.1")).0;
    assert_eq!(c_new, c_old);
    // A choice made on the new id is kept on the next build.
    conn.execute("UPDATE comp_marks SET fav = 0 WHERE comp_key = ?", [&a_new]).unwrap();
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    assert_eq!(q(&conn, &format!("SELECT fav FROM comp_marks WHERE comp_key = '{a_new}'")), 0);
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What POST /api/lists/mark { key, fav: true } does.
fn routes_like_mark(conn: &Connection, key: &str) {
    conn.execute("INSERT OR IGNORE INTO comp_marks (comp_key) VALUES (?)", [key]).unwrap();
    conn.execute("UPDATE comp_marks SET fav = 1 WHERE comp_key = ?", [key]).unwrap();
}

/// Renames and merges are applied on every rebuild, full or incremental, under either method, and an
/// incremental build with them equals a full one.
#[test]
fn renames_and_merges_are_applied_after_every_rebuild() {
    for method in [Method::Clusters, Method::Baseline] {
        let tag = method.as_str();
        let boards = fixture(21, 80, &six_comps());
        let more = fixture(22, 40, &six_comps());
        let (dir, mut conn) = fresh_db(&format!("ovr-{tag}"));
        set_comp_method(&conn, method).unwrap();
        let ids = put(&conn, "91.1", now_ms() - 3_600_000, 0, &boards);
        build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
        let (a, b, c) = if method == Method::Clusters {
            let mem = members(&conn, "91.1");
            (majority(&ids, &boards, "A", &mem).0, majority(&ids, &boards, "B", &mem).0, majority(&ids, &boards, "C", &mem).0)
        } else {
            ("T_A|TFT99_A7".to_string(), "T_B|TFT99_B7".to_string(), "T_C|TFT99_C5".to_string())
        };
        let games = |conn: &Connection, k: &str| -> i64 {
            conn.query_row("SELECT COALESCE(SUM(games), 0) FROM stat_comps WHERE bucket = 'ALL' AND comp_key = ?", [k], |r| r.get(0)).unwrap()
        };
        let (ga, gb) = (games(&conn, &a), games(&conn, &b));
        assert!(ga > 0 && gb > 0, "{tag}: {a} {ga}, {b} {gb}");

        overrides::merge(&conn, &b, Some(&a)).unwrap();
        overrides::rename(&conn, &c, "  Cee  ").unwrap();
        assert!(overrides::merge(&conn, &a, Some(&b)).is_err(), "{tag}: a circle is refused");
        assert!(overrides::merge(&conn, &a, Some(&a)).is_err());
        // The merge marked the scopes stale: an incremental build with no new matches still applies it.
        let touched = build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(touched.len(), 8, "{tag}");
        assert_eq!(games(&conn, &b), 0, "{tag}: B is merged away");
        assert_eq!(games(&conn, &a), ga + gb, "{tag}: A has B's boards");
        assert_eq!(overrides::name_of(&conn, &c).unwrap().as_deref(), Some("Cee"));

        // More boards, an incremental build, and a full one: the merge holds and both agree.
        put(&conn, "91.1", now_ms() - 3_500_000, 500, &more);
        build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert_eq!(games(&conn, &b), 0, "{tag}: still merged after an incremental build");
        let (fresh_dir, mut fresh) = fresh_db(&format!("ovr-{tag}-fresh"));
        set_comp_method(&fresh, method).unwrap();
        put(&fresh, "91.1", now_ms() - 3_600_000, 0, &boards);
        put(&fresh, "91.1", now_ms() - 3_500_000, 500, &more);
        overrides::merge(&fresh, &b, Some(&a)).unwrap();
        build_stats_with(&mut fresh, 2, &mut |_| {}).unwrap();
        drop(fresh);
        assert_same_stats(&conn, &fresh_dir.join("t.db"));

        // Undoing the merge brings B back on the next build.
        overrides::clear(&conn, &b).unwrap();
        build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap();
        assert!(games(&conn, &b) > 0, "{tag}: unmerged");
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&fresh_dir).unwrap();
    }
}

#[test]
fn the_baseline_is_the_default_and_switching_rebuilds_everything() {
    let (dir, mut conn) = fresh_db("method");
    assert_eq!(comp_method(&conn).unwrap(), Method::Baseline);
    put(&conn, "91.1", now_ms() - 3_600_000, 0, &fixture(1, 30, &six_comps()));
    build_stats_with(&mut conn, 2, &mut |_| {}).unwrap();
    assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE core IS NOT NULL"), 0, "the baseline leaves core empty");
    assert!(set_comp_method(&conn, Method::Clusters).unwrap());
    assert!(!set_comp_method(&conn, Method::Clusters).unwrap(), "no change the second time");
    let raw: String = conn.query_row("SELECT value FROM kv WHERE key = 'comp_method'", [], |r| r.get(0)).unwrap();
    assert_eq!(raw, "\"clusters\"");
    assert_eq!(build_stats_incremental_with(&mut conn, 2, &mut |_| {}).unwrap().len(), 8, "every scope is rebuilt");
    assert_eq!(q(&conn, "SELECT COUNT(*) FROM stat_comps WHERE bucket = 'ALL' AND comp_key LIKE '%|%'"), 0);
    assert_eq!(Method::parse("nonsense"), None);
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// PRD 2 speed target: clustering one scope of 8 million boards in under 2 minutes. On demand:
/// `cargo test --release -p tft-stats -- --ignored --nocapture eight_million`. `HERTZ_BENCH_BOARDS` sets the
/// number of boards. Times the clustering itself on boards in memory; reading them out of SQLite is timed
/// separately on a tenth of them and scaled.
#[test]
#[ignore]
fn eight_million_boards_cluster_in_under_two_minutes() {
    let n: usize = std::env::var("HERTZ_BENCH_BOARDS").ok().and_then(|v| v.parse().ok()).unwrap_or(8_000_000);
    // 60 units, 40 archetypes of 8 units with popularity skewed, a random swap or extra unit, two carries.
    let mut rng = Rng(42);
    let t = Instant::now();
    let make = |rng: &mut Rng, b: &mut BoardsBuilder, id: i64| {
        let a = (rng.below(40) * rng.below(40)) / 40;
        let size = 7 + rng.below(3) as i64;
        let carry = rng.below(2) as i64;
        for j in 0..size {
            let unit = if j == size - 1 && rng.below(3) == 0 { rng.below(60) as i64 } else { (a as i64 * 7 + j * 3) % 60 };
            let items = if j == carry { 3 } else { rng.below(2) as i64 };
            b.push(id, 1 + id % 8, &format!("U{unit:02}"), 1 + rng.below(3) as i64, items, 1 + unit % 5);
        }
    };
    let mut b = BoardsBuilder::default();
    for id in 0..n as i64 {
        make(&mut rng, &mut b, id);
    }
    let boards: Boards = b.finish();
    let built = t.elapsed();
    let t = Instant::now();
    let min_games = COMP_MIN_GAMES.max(((n as f64) * COMP_MIN_SHARE).ceil() as i64) as u32;
    let labels = cluster(boards, min_games, COMP_LIMIT as usize);
    let took = t.elapsed();
    let covered = labels.board_comp.iter().filter(|c| c.is_some()).count();
    println!(
        "{n} boards: made in memory {:.1} s; clustered in {:.1} s into {} comps ({:.1}% of boards in a listed comp)",
        built.as_secs_f64(),
        took.as_secs_f64(),
        labels.comps.len(),
        covered as f64 * 100.0 / n as f64
    );

    // Reading boards out of the database, on a tenth of them.
    let (dir, mut conn) = clusters_db("bench8m");
    let tenth = (n / 10).max(1);
    let tx = conn.transaction().unwrap();
    tx.execute_batch("CREATE TEMP TABLE p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT)").unwrap();
    {
        let mut pp = tx.prepare("INSERT INTO p VALUES (?, ?, 8, NULL)").unwrap();
        let mut uu = tx.prepare("INSERT INTO units VALUES (?, ?, ?, ?, NULL, ?)").unwrap();
        let mut rng = Rng(7);
        for id in 1..=tenth as i64 {
            pp.execute(params![id, 1 + id % 8]).unwrap();
            let a = (rng.below(40) * rng.below(40)) / 40;
            for j in 0..8i64 {
                let unit = (a as i64 * 7 + j * 3) % 60;
                uu.execute(params![id, format!("U{unit:02}"), 2, if j == 0 { 3 } else { 0 }, RARITY[(unit % 5) as usize]]).unwrap();
            }
        }
    }
    let t = Instant::now();
    let loaded = Boards::load(&tx, "p").unwrap();
    let load = t.elapsed();
    let t = Instant::now();
    let min_games = COMP_MIN_GAMES.max(((tenth as f64) * COMP_MIN_SHARE).ceil() as i64);
    drop(loaded);
    cluster::label(&tx, min_games, COMP_LIMIT).unwrap();
    let whole = t.elapsed();
    tx.rollback().unwrap();
    println!(
        "from SQLite, {tenth} boards: read {:.1} s, read + cluster + write labels {:.1} s (x10 for {n}: about {:.0} s)",
        load.as_secs_f64(),
        whole.as_secs_f64(),
        whole.as_secs_f64() * 10.0
    );
    assert!(took.as_secs() < 120, "clustering took {:.1} s", took.as_secs_f64());
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}
