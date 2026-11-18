//! The index file and its refreshes, on hand-made boards.

use crate::{Answer, Builder, Index, Kind, Query, RefreshKind, key, refresh};
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
