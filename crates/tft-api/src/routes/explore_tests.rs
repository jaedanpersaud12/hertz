//! PRD 4 X5: the Explorer index gives exactly the SQL joins' answers, and odd questions never break either.
//!
//! The fixture is made to be awkward for an index: two patches, units that sit twice on one board, items held by a
//! unit that is not on the board, traits at tier 0, matches from other queues and from before queues were recorded,
//! a few match times before 1970, levels and comps not known, more units and items than the answer lists (so the
//! cut at 40 partners and 30 items falls among ties), and ids that differ only in letter case or need escaping.

use super::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// xorshift64*: the same boards and questions every run.
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

pub(crate) const PATCHES: [&str; 2] = ["16.1", "16.2"];

fn unit_names() -> Vec<String> {
    let mut v: Vec<String> = (0..45).map(|i| format!("TFT16_U{i:02}")).collect();
    v.extend(["TFT16_u01", "TFT16_Ünit", "a|b", "X", "TFT16_U01 x"].map(String::from));
    v
}
fn item_names() -> Vec<String> {
    let mut v: Vec<String> = (0..38).map(|i| format!("TFT_Item_{i:02}")).collect();
    v.extend(["TFT_Item_ítem", "tft_item_00"].map(String::from));
    v
}
fn trait_names() -> Vec<String> {
    let mut v: Vec<String> = (0..12).map(|i| format!("TFT16_T{i:02}")).collect();
    v.extend(["TFT16_t00", "TFT16_Ünique"].map(String::from));
    v
}

/// Mostly one of the first `common`, sometimes any.
fn biased(r: &mut Rng, n: usize, common: usize) -> usize {
    if r.below(4) == 0 { r.below(n as u64) as usize } else { r.below(common as u64) as usize }
}

/// A database of `matches` matches per patch, with stat_meta rows so every scope resolves.
pub(crate) fn fixture(dir: &Path, matches: usize, seed: u64) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("tft.db");
    let mut conn = tft_store::open(&path).unwrap();
    let (units, items, traits) = (unit_names(), item_names(), trait_names());
    let mut r = Rng::new(seed);
    let now = now_ms();
    let tx = conn.transaction().unwrap();
    let mut pid = 0i64;
    for patch in PATCHES {
        for m in 0..matches {
            let id = format!("{patch}_M{m}");
            let rank = r.below(10) as i64;
            let queue: Option<i64> = *r.pick(&[None, Some(1100), Some(1100), Some(1100), Some(1090)]);
            let time = if r.below(50) == 0 { -5 - r.below(1000) as i64 } else { now - r.below(10 * 86_400_000) as i64 };
            tx.execute(
                "INSERT INTO matches (match_id, platform, patch, set_number, game_datetime, game_length, rank, queue_id) VALUES (?1, 'na1', ?2, 16, ?3, 1800, ?4, ?5)",
                rusqlite::params![id, patch, time, rank, queue],
            )
            .unwrap();
            for slot in 0..8 {
                pid += 1 + r.below(3) as i64; // ids with gaps
                let level: Option<i64> = (r.below(10) != 0).then(|| 3 + r.below(8) as i64);
                let comp: Option<String> = (r.below(10) != 0).then(|| format!("C{}|{}", r.below(6), units[r.below(8) as usize]));
                tx.execute(
                    "INSERT INTO participants (id, match_id, puuid, placement, level, last_round, gold_left, damage, comp_key) VALUES (?1, ?2, ?3, ?4, ?5, 30, 0, 50, ?6)",
                    rusqlite::params![pid, id, format!("p{pid}"), slot + 1, level, comp],
                )
                .unwrap();
                let n = 2 + r.below(9);
                let mut on_board = Vec::new();
                for _ in 0..n {
                    let u = biased(&mut r, units.len(), 14);
                    on_board.push(u);
                    tx.execute(
                        "INSERT INTO units (participant_id, character_id, star, n_items, build, rarity) VALUES (?1, ?2, ?3, ?4, NULL, 0)",
                        rusqlite::params![pid, units[u], 1 + r.below(3) as i64, r.below(4) as i64],
                    )
                    .unwrap();
                    for _ in 0..r.below(4) {
                        tx.execute(
                            "INSERT INTO unit_items (participant_id, character_id, item) VALUES (?1, ?2, ?3)",
                            rusqlite::params![pid, units[u], items[biased(&mut r, items.len(), 12)]],
                        )
                        .unwrap();
                    }
                }
                if r.below(20) == 0 {
                    // An item row for a unit that is not on the board.
                    let away = (0..units.len()).find(|u| !on_board.contains(u)).unwrap();
                    tx.execute(
                        "INSERT INTO unit_items (participant_id, character_id, item) VALUES (?1, ?2, ?3)",
                        rusqlite::params![pid, units[away], items[r.below(items.len() as u64) as usize]],
                    )
                    .unwrap();
                }
                for _ in 0..r.below(8) {
                    tx.execute(
                        "INSERT INTO traits (participant_id, name, num_units, tier_current, style) VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![pid, traits[biased(&mut r, traits.len(), 8)], 1 + r.below(9) as i64, r.below(5) as i64, r.below(5) as i64],
                    )
                    .unwrap();
                }
            }
        }
        for b in &BUCKETS {
            for w in WINDOWS {
                tx.execute(
                    "INSERT INTO stat_meta (patch, bucket, matches, comps, built_at) VALUES (?1, ?2, ?3, ?4, 1760000000000)",
                    rusqlite::params![patch, scope_key(b.key, w.0), matches as i64, (matches * 8) as i64],
                )
                .unwrap();
            }
        }
    }
    tx.commit().unwrap();
    path
}

/// Fresh, empty scratch folder for one test.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hertz-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One index per fixture patch, built the way the stats step builds them.
pub(crate) fn indexes(db: &Path) -> HashMap<String, Arc<tft_explore::Index>> {
    let conn = tft_store::open_readonly(db).unwrap();
    let dir = tft_explore::index_dir(db);
    PATCHES
        .iter()
        .map(|p| {
            let r = tft_explore::refresh(&conn, &dir, p).unwrap();
            (p.to_string(), Arc::new(tft_explore::Index::open(&r.path.unwrap()).unwrap()))
        })
        .collect()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// A random question: up to five units and four traits (more than the route reads), unknown and odd ids, spaces
/// and empty entries, every scope and some that do not exist.
fn question(r: &mut Rng) -> String {
    let (units, traits) = (unit_names(), trait_names());
    let ids = |pool: &[String], max: u64, usual: u64, r: &mut Rng| -> String {
        let mut parts = Vec::new();
        let n = if r.below(4) == 0 { r.below(max + 1) } else { usual + r.below(2) };
        for _ in 0..n {
            parts.push(match r.below(20) {
                0 => "nope".to_string(),
                1 => format!(" {} ", pool[r.below(3) as usize]),
                2 => String::new(),
                _ => pool[biased(r, pool.len(), 10)].clone(),
            });
        }
        parts.iter().map(|p| enc(p)).collect::<Vec<_>>().join(",")
    };
    let u = ids(&units, 5, 1, r);
    let t = ids(&traits, 4, 0, r);
    let mut q = vec![format!("units={u}")];
    if !t.is_empty() || r.below(2) == 0 {
        q.push(format!("traits={t}"));
    }
    let patch = *r.pick(&["", "", "16.1", "16.2", "nope"]);
    if !patch.is_empty() {
        q.push(format!("patch={patch}"));
    }
    let bucket = *r.pick(&["", "ALL", "PLATINUM%2B", "DIAMOND%2B", "MASTER%2B", "GOLD"]);
    if !bucket.is_empty() {
        q.push(format!("bucket={bucket}"));
    }
    let window = *r.pick(&["", "", "3d", "7d"]);
    if !window.is_empty() {
        q.push(format!("window={window}"));
    }
    q.join("&")
}

fn without_ms(mut o: Obj) -> Obj {
    o.remove("ms");
    o
}

#[test]
fn the_index_gives_the_sql_answers_to_800_random_questions() {
    let dir = scratch("explore-equivalence");
    let db = fixture(&dir, 150, 7);
    let ix = indexes(&db);
    let conn = tft_store::open_readonly(&db).unwrap();
    let mut r = Rng::new(42);
    let (mut from_index, mut found, mut cut) = (0, 0, 0);
    for n in 0..800 {
        let q = question(&mut r);
        let (sql, s1) = explore_using(&conn, Some(&q), &|_| None).unwrap();
        let (fast, s2) = explore_using(&conn, Some(&q), &|p| ix.get(p).cloned()).unwrap();
        assert_eq!(without_ms(fast.clone()), without_ms(sql.clone()), "question {n}: {q}");
        assert!(s1 != Source::Index, "{q}");
        if s2 == Source::Index {
            from_index += 1;
            assert_eq!(s1, Source::Sql);
        }
        if sql["summary"]["games"].as_i64().unwrap_or(0) > 0 {
            found += 1;
        }
        if sql["items"].as_array().is_some_and(|i| i.len() == 30) || sql["partners"].as_array().is_some_and(|p| p.len() >= 36) {
            cut += 1;
        }
    }
    // The questions must have exercised the index, found boards, and hit the 30-item and 40-partner cut-offs.
    assert!(from_index >= 500, "only {from_index} of 800 answered from the index");
    assert!(found >= 300, "only {found} of 800 found boards");
    assert!(cut >= 20, "only {cut} answers reached a cut-off");
    eprintln!("{from_index} of 800 from the index, {found} found boards, {cut} at a cut-off");
    let _ = std::fs::remove_dir_all(dir);
}

/// Malformed query strings: random bytes, broken escapes, NULs, huge lists. Both ways answer, and agree.
#[test]
fn malformed_questions_never_fail_and_both_ways_agree() {
    let dir = scratch("explore-fuzz");
    let db = fixture(&dir, 30, 11);
    let ix = indexes(&db);
    let conn = tft_store::open_readonly(&db).unwrap();
    let mut r = Rng::new(9);
    let alphabet: Vec<&str> = vec![
        "units", "traits", "patch", "bucket", "window", "min", "=", "&", ",", ",", "%", "%2", "%E0%A4%A", "%2C", "%00", "+", " ", "\u{feff}",
        "\u{85}", "|", "TFT16_U01", "TFT16_U02", "TFT16_T01", "16.1", "16.2", "3d", "MASTER%2B", "é", "\u{0}", "%FF", "🙂", "units=", "&units=",
    ];
    for n in 0..1500 {
        let mut q = String::new();
        for _ in 0..r.below(40) {
            q.push_str(r.pick(&alphabet));
        }
        if n % 50 == 0 {
            q = format!("units={}", vec!["TFT16_U01"; 5000].join(","));
        }
        let a = explore_using(&conn, Some(&q), &|_| None);
        let b = explore_using(&conn, Some(&q), &|p| ix.get(p).cloned());
        let (Ok((a, _)), Ok((b, _))) = (a, b) else { panic!("question {n} failed: {q:?}") };
        assert_eq!(without_ms(a), without_ms(b), "question {n}: {q:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Missing, cut-short and old-format files: the Explorer answers from SQL, then builds a file in the background
/// and answers from it.
#[test]
fn a_missing_or_old_index_falls_back_to_sql_and_is_rebuilt() {
    let dir = scratch("explore-fallback");
    let db = fixture(&dir, 20, 3);
    let conn = tft_store::open_readonly(&db).unwrap();
    let q = "units=TFT16_U01&patch=16.2";
    let (want, _) = explore_using(&conn, Some(q), &|_| None).unwrap();
    let idx_dir = tft_explore::index_dir(&db);

    for damage in ["missing", "old format", "cut short"] {
        let _ = std::fs::remove_dir_all(&idx_dir);
        if damage != "missing" {
            let path = tft_explore::refresh(&conn, &idx_dir, "16.2").unwrap().path.unwrap();
            let mut bytes = std::fs::read(&path).unwrap();
            if damage == "old format" {
                bytes[8] = 0; // format version 0
            } else {
                bytes.truncate(bytes.len() - 7);
            }
            std::fs::write(&path, bytes).unwrap();
            assert!(tft_explore::Index::open(&path).is_err(), "{damage}: the file must be refused");
        }
        let explorer = tft_explore::Explorer::new(&db, tft_explore::Mode::Background);
        let (first, source) = explore_using(&conn, Some(q), &|p| explorer.get(p)).unwrap();
        assert_eq!(source, Source::Sql, "{damage}: the first answer comes from SQL");
        assert_eq!(without_ms(first), without_ms(want.clone()), "{damage}");
        let mut source = Source::Sql;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let (again, s) = explore_using(&conn, Some(q), &|p| explorer.get(p)).unwrap();
            assert_eq!(without_ms(again), without_ms(want.clone()), "{damage}");
            source = s;
            if s == Source::Index {
                break;
            }
        }
        assert_eq!(source, Source::Index, "{damage}: rebuilt in the background, then used");
        assert_eq!(tft_explore::generations(&idx_dir, "16.2").len(), 1, "{damage}: the bad file is replaced");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// While a new snapshot is written (boards added since), a question keeps the snapshot it started with; the next
/// question after the new file is complete sees the new boards.
#[test]
fn questions_see_the_last_complete_snapshot() {
    let dir = scratch("explore-snapshot");
    let db = fixture(&dir, 20, 5);
    let idx_dir = tft_explore::index_dir(&db);
    let explorer = tft_explore::Explorer::new(&db, tft_explore::Mode::Wait);
    let q = "units=TFT16_U01&patch=16.2";
    let read = tft_store::open_readonly(&db).unwrap();
    let games = |b: &Obj| b["summary"]["games"].as_i64().unwrap();
    let (before, s) = explore_using(&read, Some(q), &|p| explorer.get(p)).unwrap();
    assert_eq!(s, Source::Index);
    let held = explorer.get("16.2").unwrap(); // a question in flight holds this snapshot

    // New boards with the unit, then a stats-step refresh: boards are appended to a new file.
    {
        let w = tft_store::open(&db).unwrap();
        w.execute("INSERT INTO matches (match_id, platform, patch, game_datetime, rank, queue_id) VALUES ('NEW', 'na1', '16.2', ?1, 9, 1100)", [now_ms()]).unwrap();
        for k in 0..3 {
            w.execute("INSERT INTO participants (id, match_id, puuid, placement, comp_key) VALUES (?1, 'NEW', 'x', 1, NULL)", [900_000 + k]).unwrap();
            w.execute("INSERT INTO units (participant_id, character_id, star, n_items) VALUES (?1, 'TFT16_U01', 2, 0)", [900_000 + k]).unwrap();
        }
        let r = tft_explore::refresh(&w, &idx_dir, "16.2").unwrap();
        assert_eq!((r.kind, r.added), (tft_explore::RefreshKind::Appended, 3));
    }
    // The held snapshot still answers as before, even though its file has been replaced.
    let old = held.explore(&tft_explore::Query {
        units: &["TFT16_U01".to_string()],
        traits: &[],
        rank_min: 0,
        since: 0,
        ranked_queue: 1100,
        partner_limit: 40,
        item_limit: 30,
    });
    assert_eq!(old.summary.games, games(&before));
    // A new question picks up the new snapshot (after the one-second rescan) and matches SQL.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (after, s) = explore_using(&read, Some(q), &|p| explorer.get(p)).unwrap();
    assert_eq!(s, Source::Index);
    assert_eq!(games(&after), games(&before) + 3);
    let (sql, _) = explore_using(&read, Some(q), &|_| None).unwrap();
    assert_eq!(without_ms(after), without_ms(sql));
    // A half-written file (a writer that stopped) is never picked up.
    std::fs::write(idx_dir.join("16.2.00000099.hzx.tmp-1"), b"HERTZIDX partial").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(explore_using(&read, Some(q), &|p| explorer.get(p)).unwrap().1, Source::Index);
    let _ = std::fs::remove_dir_all(dir);
}
