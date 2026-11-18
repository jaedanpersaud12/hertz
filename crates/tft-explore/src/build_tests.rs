//! The index file and its refreshes, on hand-made boards.

use crate::{Answer, Builder, Index, Kind, Query, RefreshKind, Row, key, refresh};
use rusqlite::Connection;
use std::path::PathBuf;

fn db(name: &str) -> (Connection, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hertz-explore-build-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let conn = tft_store::open(&dir.join("tft.db")).unwrap();
    (conn, dir)
}

type Units<'a> = &'a [(&'a str, i64)];
type Items<'a> = &'a [(&'a str, &'a str)];

/// One board: placement, units (id, star), items (unit, item), traits (name, tier).
fn board(c: &Connection, pid: i64, patch: &str, placement: i64, units: Units, items: Items, traits: Units) {
    let m = format!("M{pid}");
    c.execute(
        "INSERT INTO matches (match_id, platform, patch, game_datetime, rank, queue_id) VALUES (?1, 'na1', ?2, 1000, 8, 1100)",
        rusqlite::params![m, patch],
    )
    .unwrap();
    c.execute(
        "INSERT INTO participants (id, match_id, puuid, placement, level) VALUES (?1, ?2, 'p', ?3, 8)",
        rusqlite::params![pid, m, placement],
    )
    .unwrap();
    for (u, s) in units {
        c.execute("INSERT INTO units (participant_id, character_id, star, n_items) VALUES (?1, ?2, ?3, 0)", rusqlite::params![pid, u, s])
            .unwrap();
    }
    for (u, i) in items {
        c.execute("INSERT INTO unit_items (participant_id, character_id, item) VALUES (?1, ?2, ?3)", rusqlite::params![pid, u, i]).unwrap();
    }
    for (t, tier) in traits {
        c.execute(
            "INSERT INTO traits (participant_id, name, num_units, tier_current, style) VALUES (?1, ?2, 2, ?3, 1)",
            rusqlite::params![pid, t, tier],
        )
        .unwrap();
    }
}

fn ask<'a>(units: &'a [String], traits: &'a [String]) -> Query<'a> {
    Query { units, traits, rank_min: 0, since: 0, ranked_queue: 1100, partner_limit: 40, item_limit: 30 }
}

fn names(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_file_holds_every_board_and_its_bitmaps() {
    let (c, dir) = db("roundtrip");
    board(&c, 1, "1.1", 1, &[("A", 2), ("B", 1), ("A", 3)], &[("A", "I1"), ("A", "I1"), ("B", "I2")], &[("T", 1), ("T", 0)]);
    board(&c, 3, "1.1", 5, &[("B", 2)], &[], &[]);
    board(&c, 4, "2.1", 2, &[("A", 1)], &[], &[]); // another patch
    board(&c, 7, "1.1", 3, &[], &[("A", "I3")], &[("T", 2)]); // an item for a unit not on the board
    let r = refresh(&c, &dir, "1.1").unwrap();
    assert_eq!((r.kind, r.rows), (RefreshKind::Full, 3));
    let ix = Index::open(r.path.as_ref().unwrap()).unwrap();
    assert_eq!(ix.rows().iter().map(|r| (r.pid, r.placement)).collect::<Vec<_>>(), [(1, 1), (3, 5), (7, 3)]);
    let rows_of = |k| ix.bitmap(k).iter().collect::<Vec<u32>>();
    let (a, b, t) = (ix.unit_id("A").unwrap() as u32, ix.unit_id("B").unwrap() as u32, ix.trait_id("T").unwrap() as u32);
    assert_eq!(rows_of(key(Kind::Unit, a, 0)), [0]);
    assert_eq!(rows_of(key(Kind::Unit, b, 0)), [0, 1]);
    assert_eq!(rows_of(key(Kind::UnitStar, a, 2)), [0]);
    assert_eq!(rows_of(key(Kind::UnitStar, a, 3)), [0]);
    assert!(rows_of(key(Kind::UnitStar, a, 1)).is_empty());
    assert_eq!(rows_of(key(Kind::UnitItem, a, ix.item_id("I1").unwrap() as u32)), [0]);
    assert_eq!(rows_of(key(Kind::UnitItem, a, ix.item_id("I3").unwrap() as u32)), [2]);
    assert_eq!(rows_of(key(Kind::Item, ix.item_id("I2").unwrap() as u32, 0)), [0]);
    assert_eq!(rows_of(key(Kind::Trait, t, 0)), [0, 2]);
    assert_eq!(rows_of(key(Kind::TraitTier, t, 0)), [0]);
    assert_eq!(rows_of(key(Kind::TraitTier, t, 2)), [2]);
    assert_eq!(rows_of(key(Kind::Placement, 0, 5)), [1]);
    assert_eq!(rows_of(key(Kind::Level, 0, 8)), [0, 1, 2]);

    // A unit twice on a board and an item held twice count once.
    let a_only = ix.explore(&ask(&names(&["A"]), &[]));
    assert_eq!(a_only.summary.games, 1);
    assert_eq!(a_only.partners.iter().map(|(n, c)| (n.as_str(), c.games)).collect::<Vec<_>>(), [("B", 1), ("A", 1)]);
    assert_eq!(a_only.items.iter().map(|(n, c)| (n.as_str(), c.games)).collect::<Vec<_>>(), [("I1", 1)]);
    assert_eq!(ix.explore(&ask(&names(&["A"]), &names(&["T"]))).summary.games, 1);
    assert_eq!(ix.explore(&ask(&names(&["nope"]), &[])), Answer::default());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_refresh_adds_new_boards_and_rebuilds_when_older_ones_change() {
    let (c, dir) = db("refresh");
    board(&c, 1, "1.1", 1, &[("A", 2)], &[], &[]);
    board(&c, 2, "1.1", 8, &[("A", 1), ("B", 1)], &[], &[]);
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Full);
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Current, "nothing new: the file stays");

    board(&c, 5, "1.1", 4, &[("C", 1), ("A", 3)], &[("C", "I")], &[("T", 1)]);
    let r = refresh(&c, &dir, "1.1").unwrap();
    assert_eq!((r.kind, r.added, r.rows), (RefreshKind::Appended, 1, 3));
    assert_eq!(crate::generations(&dir, "1.1").len(), 1, "the older snapshot is removed");
    let appended = Index::open(r.path.as_ref().unwrap()).unwrap();

    // The appended file answers as a file built from scratch does.
    let mut b = Builder::new("1.1");
    b.add_from_db(&c, i64::MIN).unwrap();
    let fresh = Index::open(&b.write(&dir.join("fresh")).unwrap()).unwrap();
    for (u, t) in [(vec!["A"], vec![]), (vec!["C"], vec!["T"]), (vec!["B", "A"], vec![]), (vec!["C"], vec![])] {
        let (u, t) = (names(&u), names(&t));
        assert_eq!(appended.explore(&ask(&u, &t)), fresh.explore(&ask(&u, &t)), "{u:?} {t:?}");
    }

    // A board deleted (pruned), or one moved into the patch below the newest id: built whole again.
    c.execute("DELETE FROM participants WHERE id = 1", []).unwrap();
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Full);
    board(&c, 0, "9.9", 2, &[("A", 1)], &[], &[]);
    c.execute("UPDATE matches SET patch = '1.1' WHERE match_id = 'M0'", []).unwrap();
    let r = refresh(&c, &dir, "1.1").unwrap();
    assert_eq!((r.kind, r.rows), (RefreshKind::Full, 3));
    // No boards left: no file.
    c.execute("DELETE FROM matches", []).unwrap();
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Empty);
    assert!(crate::generations(&dir, "1.1").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn values_the_index_cannot_hold_exactly_refuse_the_build() {
    let (c, dir) = db("refuse");
    board(&c, 1, "1.1", 1, &[("A", 2)], &[], &[]);
    c.execute("UPDATE participants SET placement = 1.5", []).unwrap();
    assert!(refresh(&c, &dir, "1.1").is_err(), "a placement that is not a whole number");
    c.execute("UPDATE participants SET placement = 70000", []).unwrap();
    assert!(refresh(&c, &dir, "1.1").is_err(), "a placement too large for the row");
    c.execute("UPDATE participants SET placement = 1", []).unwrap();
    c.execute("UPDATE units SET star = 300", []).unwrap();
    assert!(refresh(&c, &dir, "1.1").is_err(), "a star too large for the entry");
    c.execute("UPDATE units SET star = 2, character_id = CAST(X'FF' AS TEXT)", []).unwrap();
    assert!(refresh(&c, &dir, "1.1").is_err(), "an id that is not UTF-8");
    assert!(crate::generations(&dir, "1.1").is_empty(), "nothing written");
    c.execute("UPDATE units SET character_id = 'A'", []).unwrap();
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Full);
    let _ = std::fs::remove_dir_all(dir);
}

/// xorshift64: the same boards and questions every run.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

/// `n` random boards with ids from `from`, in patch 1.1 (some ranks, times, queues, duplicates and odd items).
fn random_boards(c: &Connection, r: &mut Rng, from: i64, n: i64) {
    for pid in from..from + n {
        let m = format!("M{pid}");
        let queue: Option<i64> = [None, Some(1100), Some(1100), Some(1090)][r.below(4) as usize];
        c.execute(
            "INSERT INTO matches (match_id, platform, patch, game_datetime, rank, queue_id) VALUES (?1, 'na1', '1.1', ?2, ?3, ?4)",
            rusqlite::params![m, 1000 + r.below(50) as i64, r.below(10) as i64, queue],
        )
        .unwrap();
        let level: Option<i64> = (r.below(8) != 0).then(|| 3 + r.below(7) as i64);
        let comp: Option<String> = (r.below(6) != 0).then(|| format!("C{}", r.below(5)));
        c.execute(
            "INSERT INTO participants (id, match_id, puuid, placement, level, comp_key) VALUES (?1, ?2, 'p', ?3, ?4, ?5)",
            rusqlite::params![pid, m, 1 + r.below(8) as i64, level, comp],
        )
        .unwrap();
        // Later batches bring in units, traits and items earlier ones never had.
        let pool = 6 + (pid / 40) as u64;
        for _ in 0..r.below(7) {
            let u = format!("U{}", r.below(pool));
            c.execute(
                "INSERT INTO units (participant_id, character_id, star, n_items) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![pid, u, 1 + r.below(3) as i64, r.below(4) as i64],
            )
            .unwrap();
            for _ in 0..r.below(3) {
                c.execute(
                    "INSERT INTO unit_items (participant_id, character_id, item) VALUES (?1, ?2, ?3)",
                    rusqlite::params![pid, u, format!("I{}", r.below(pool))],
                )
                .unwrap();
            }
        }
        if r.below(10) == 0 {
            c.execute(
                "INSERT INTO unit_items (participant_id, character_id, item) VALUES (?1, ?2, 'I0')",
                rusqlite::params![pid, format!("Only{}", pid % 3)],
            )
            .unwrap();
        }
        for _ in 0..r.below(5) {
            c.execute(
                "INSERT INTO traits (participant_id, name, num_units, tier_current, style) VALUES (?1, ?2, 2, ?3, 1)",
                rusqlite::params![pid, format!("T{}", r.below(pool / 2)), r.below(4) as i64],
            )
            .unwrap();
        }
    }
}

type Content = (Vec<String>, std::collections::BTreeMap<String, Vec<i64>>);

/// Every board as names and values, and every bitmap as (kind, names, value) -> participant ids: the file's
/// content without its dictionary ids, which a file built in steps may number differently.
fn content(ix: &Index) -> Content {
    let d = &ix.dir;
    let rows = ix.rows();
    let boards = (0..rows.len())
        .map(|r| {
            let row = &rows[r];
            let mut u: Vec<String> = crate::file::entries(rows, r, ix.units(), |x| x.unit_start)
                .iter()
                .map(|&e| {
                    let (id, star, n) = crate::unit_parts(e);
                    format!("{}*{star}+{n}", d.units[id as usize])
                })
                .collect();
            let mut t: Vec<String> = crate::file::entries(rows, r, ix.traits(), |x| x.trait_start)
                .iter()
                .map(|&e| {
                    let (id, tier) = crate::trait_parts(e);
                    format!("{}@{tier}", d.traits[id as usize])
                })
                .collect();
            let mut i: Vec<String> = crate::file::entries(rows, r, ix.items(), |x| x.item_start)
                .iter()
                .map(|&e| format!("{}:{}", d.units[(e >> 16) as usize], d.items[(e & 0xffff) as usize]))
                .collect();
            u.sort();
            t.sort();
            i.sort();
            let comp = if row.comp == Row::NO_COMP { "-".to_string() } else { d.comps[row.comp as usize].clone() };
            format!(
                "{} t{} p{} l{} r{} q{:?} c{comp} {u:?} {t:?} {i:?}",
                row.pid, row.time, row.placement, row.level, row.rank, d.queues[row.queue as usize]
            )
        })
        .collect();
    let mut bitmaps = std::collections::BTreeMap::new();
    let kinds = [
        Kind::Unit, Kind::UnitStar, Kind::UnitItem, Kind::Trait, Kind::TraitTier, Kind::Item, Kind::Level, Kind::Placement,
        Kind::Rank, Kind::Queue, Kind::Comp,
    ];
    for kind in kinds {
        for (a, b) in ix.keys(kind).collect::<Vec<_>>() {
            let name = match kind {
                Kind::Unit | Kind::UnitStar => format!("{kind:?} {} {b}", d.units[a as usize]),
                Kind::UnitItem => format!("{kind:?} {} {}", d.units[a as usize], d.items[b as usize]),
                Kind::Trait | Kind::TraitTier => format!("{kind:?} {} {b}", d.traits[a as usize]),
                Kind::Item => format!("{kind:?} {}", d.items[a as usize]),
                Kind::Queue => format!("{kind:?} {:?}", d.queues[a as usize]),
                Kind::Comp => format!("{kind:?} {}", d.comps[a as usize]),
                _ => format!("{kind:?} {b}"),
            };
            let pids = ix.bitmap(key(kind, a, b)).iter().map(|r| rows[r as usize].pid).collect();
            bitmaps.insert(name, pids);
        }
    }
    (boards, bitmaps)
}

#[test]
fn a_file_built_in_several_appends_equals_one_built_at_once() {
    let (c, dir) = db("appends");
    let mut r = Rng(0x9e37_79b9);
    random_boards(&c, &mut r, 1, 120);
    assert_eq!(refresh(&c, &dir, "1.1").unwrap().kind, RefreshKind::Full);
    for step in 0..4 {
        random_boards(&c, &mut r, 121 + step * 70, 70);
        let a = refresh(&c, &dir, "1.1").unwrap();
        assert_eq!((a.kind, a.added), (RefreshKind::Appended, 70), "append {step}");
        assert_eq!(crate::generations(&dir, "1.1").len(), 1, "append {step}: only the newest file is left");
    }
    let stepped = Index::open(&crate::newest(&dir, "1.1").unwrap().1).unwrap();
    let mut b = Builder::new("1.1");
    b.add_from_db(&c, i64::MIN).unwrap();
    let once = Index::open(&b.write(&dir.join("once")).unwrap()).unwrap();
    assert_eq!(stepped.dir.rows, 400);
    let (sb, sm) = content(&stepped);
    let (ob, om) = content(&once);
    assert_eq!(sb, ob, "every board, as names and values");
    assert_eq!(sm, om, "every bitmap, as names and participant ids");
    assert!(sm.len() > 100, "only {} bitmaps", sm.len());

    // And answer for answer, on random questions over every scope.
    let units: Vec<String> = (0..16).map(|i| format!("U{i}")).chain(["Only0".to_string(), "nope".to_string()]).collect();
    let traits: Vec<String> = (0..8).map(|i| format!("T{i}")).collect();
    let mut found = 0;
    for _ in 0..400 {
        let us: Vec<String> = (0..1 + r.below(3)).map(|_| units[r.below(units.len() as u64) as usize].clone()).collect();
        let ts: Vec<String> = (0..r.below(2)).map(|_| traits[r.below(traits.len() as u64) as usize].clone()).collect();
        let mut q = ask(&us, &ts);
        q.rank_min = r.below(9) as i64;
        q.since = [0, 1020, 1049][r.below(3) as usize];
        q.ranked_queue = [1100, 1090][r.below(2) as usize];
        let (x, y) = (stepped.explore(&q), once.explore(&q));
        assert_eq!(x, y, "{us:?} {ts:?} rank {} since {}", q.rank_min, q.since);
        found += usize::from(x.summary.games > 0);
    }
    assert!(found > 100, "only {found} questions found boards");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_append_loads_none_of_the_old_file_and_leaves_one_file() {
    let (c, dir) = db("append-memory");
    let mut r = Rng(7);
    random_boards(&c, &mut r, 1, 50);
    let idx = dir.join("tft.db.explore");
    let first = refresh(&c, &idx, "1.1").unwrap().path.unwrap();
    // Extending a file keeps its directory and bitmap table only: no rows, arrays or bitmaps are loaded.
    let b = {
        let ix = Index::open(&first).unwrap();
        Builder::from_index(&ix)
    };
    assert_eq!((b.rows(), b.rows.len(), b.unit_e.len(), b.trait_e.len(), b.item_e.len()), (50, 0, 0, 0, 0));
    // The app has the first snapshot open while the stats step appends (one process under `tft run`).
    let held = Index::open(&first).unwrap();
    let before = held.explore(&ask(&names(&["U1"]), &[]));
    random_boards(&c, &mut r, 51, 10);
    assert_eq!(refresh(&c, &idx, "1.1").unwrap().kind, RefreshKind::Appended);
    assert_eq!(held.explore(&ask(&names(&["U1"]), &[])), before, "the held snapshot answers as before");
    drop(held);
    // Once nothing maps it, the older file is gone by the next switch (here: the app picking up the new one).
    let explorer = crate::Explorer::new(&dir.join("tft.db"), crate::Mode::Wait);
    assert_eq!(explorer.get("1.1").unwrap().dir.rows, 60);
    assert_eq!(crate::generations(&idx, "1.1").len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_time_window_includes_a_board_played_exactly_at_its_start() {
    let (c, dir) = db("window");
    board(&c, 1, "1.1", 2, &[("A", 1)], &[], &[]);
    board(&c, 2, "1.1", 6, &[("A", 1)], &[], &[]);
    c.execute("UPDATE matches SET game_datetime = 5000 WHERE match_id = 'M1'", []).unwrap();
    c.execute("UPDATE matches SET game_datetime = 4999 WHERE match_id = 'M2'", []).unwrap();
    let ix = Index::open(&refresh(&c, &dir, "1.1").unwrap().path.unwrap()).unwrap();
    let u = names(&["A"]);
    let at = |since: i64| {
        let mut q = ask(&u, &[]);
        q.since = since;
        ix.explore(&q).summary
    };
    assert_eq!((at(5000).games, at(5000).sum_place), (1, 2), "game_datetime >= since: the board at exactly 5000 counts");
    assert_eq!(at(4999).games, 2);
    assert_eq!(at(5001).games, 0);
    let _ = std::fs::remove_dir_all(dir);
}
