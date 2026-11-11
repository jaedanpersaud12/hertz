//! The database layout. Kept byte-for-byte identical to `src/db.ts`, so the TypeScript app and the Rust
//! engine can open the same file; the parity test compares `sqlite_master` from both.

pub const SCHEMA: &str = "
-- ---- Raw data from the crawler -------------------------------------------------
CREATE TABLE IF NOT EXISTS players (
  puuid        TEXT PRIMARY KEY,
  platform     TEXT NOT NULL,
  tier         TEXT NOT NULL,
  rank         INTEGER NOT NULL,
  last_crawled INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS players_next ON players(platform, last_crawled, rank DESC);

CREATE TABLE IF NOT EXISTS match_queue (
  match_id  TEXT PRIMARY KEY,
  route     TEXT NOT NULL,
  platform  TEXT NOT NULL,
  seed_rank INTEGER NOT NULL,
  status    INTEGER NOT NULL DEFAULT 0   -- 0 pending, 1 stored, 2 skipped
);
CREATE INDEX IF NOT EXISTS queue_pending ON match_queue(route, status);

CREATE TABLE IF NOT EXISTS matches (
  match_id      TEXT PRIMARY KEY,
  platform      TEXT NOT NULL,
  patch         TEXT NOT NULL,
  set_number    INTEGER,
  game_datetime INTEGER NOT NULL,
  game_length   REAL,
  rank          INTEGER NOT NULL,         -- rank of the player this match was found through
  queue_id      INTEGER                   -- Riot's queue (1100 ranked); NULL for matches stored before E12, all ranked
);
CREATE INDEX IF NOT EXISTS matches_patch ON matches(patch, rank);

CREATE TABLE IF NOT EXISTS match_stored (  -- when each match was stored, for matches per hour (E10)
  match_id TEXT PRIMARY KEY,
  platform TEXT NOT NULL,
  at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS match_stored_at ON match_stored(at, platform);

CREATE TABLE IF NOT EXISTS participants (
  id         INTEGER PRIMARY KEY,
  match_id   TEXT NOT NULL,
  puuid      TEXT NOT NULL,
  placement  INTEGER NOT NULL,
  level      INTEGER,
  last_round INTEGER,
  gold_left  INTEGER,
  damage     INTEGER,
  comp_key   TEXT                          -- \"main trait|carry unit\", see src/cluster.ts
);
CREATE INDEX IF NOT EXISTS participants_match ON participants(match_id);

CREATE TABLE IF NOT EXISTS units (
  participant_id INTEGER NOT NULL,
  character_id   TEXT NOT NULL,
  star           INTEGER NOT NULL,
  n_items        INTEGER NOT NULL,
  build          TEXT,                     -- three item ids, sorted, joined with |
  rarity         INTEGER
);
CREATE INDEX IF NOT EXISTS units_participant ON units(participant_id);
CREATE INDEX IF NOT EXISTS units_character ON units(character_id, participant_id);

CREATE TABLE IF NOT EXISTS unit_items (
  participant_id INTEGER NOT NULL,
  character_id   TEXT NOT NULL,
  item           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS unit_items_participant ON unit_items(participant_id);

CREATE TABLE IF NOT EXISTS traits (
  participant_id INTEGER NOT NULL,
  name           TEXT NOT NULL,
  num_units      INTEGER NOT NULL,
  tier_current   INTEGER NOT NULL,
  style          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS traits_participant ON traits(participant_id);

-- ---- Names, costs and icons from Community Dragon (\"npm run static\") -----------
CREATE TABLE IF NOT EXISTS static_data (
  kind TEXT NOT NULL,                      -- unit | trait | item
  id   TEXT NOT NULL,
  name TEXT NOT NULL,
  icon TEXT,
  data TEXT NOT NULL DEFAULT '{}',
  PRIMARY KEY (kind, id)
);

-- ---- Your own things -----------------------------------------------------------
CREATE TABLE IF NOT EXISTS comp_marks (comp_key TEXT PRIMARY KEY, fav INTEGER NOT NULL DEFAULT 0, hidden INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS saved_teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL, board TEXT NOT NULL, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS my_matches (match_id TEXT PRIMARY KEY, game_datetime INTEGER NOT NULL, json TEXT NOT NULL);
";

/// Numbered additions to [`SCHEMA`], applied in order every time the database is opened. Each one is idempotent
/// (`IF NOT EXISTS`), so applying it again changes nothing. Kept byte-for-byte identical to `MIGRATIONS` in
/// `src/db.ts`. New ones take the next number.
///
/// 1. Which match rows each stats scope already holds, for incremental rebuilds (E5).
/// 2. Real ranks for a sample of lobbies (E7). `rank_samples` holds one row per looked-up player: `tier` is
///    Riot's tier, or `UNRANKED` when the player has no ranked TFT entry; `division` is I to IV (empty for apex
///    tiers' single division as Riot sends it, and for unranked players). `rank_sample_queue` holds every match
///    chosen for sampling; `done` turns 1 once all its players are in `rank_samples`. Rows are never removed, so
///    choosing a match again after a restart is a no-op.
/// 3. The engine log (E10): warnings and errors from the crawler and supervisor, newest kept. See `health.rs`.
/// 4. The comp page sections (PRD 2 C5, C6, C7, C9), written by `tft-stats` (`comp_detail.rs`) per scope like the
///    other stat tables. Kept out of [`STAT_TABLES`] so a stats-version bump never drops tables only a migration
///    makes; every rebuild of a scope deletes and rewrites its rows.
/// 5. Comps you renamed or merged (PRD 2 C14), never rebuilt, and which comp of last patch each comp continues
///    (C13), rebuilt with the stats when the comp method is `clusters`. See `cluster.rs` in tft-stats.
/// 6. Flex and swaps on the comp page (PRD 2 C8), written by `tft-stats` (`flex.rs`) like migration 4's tables;
///    and the difficulty you give a comp (C10), yours and never rebuilt: `level` is easy, medium or hard.
/// 7. Unit and trait page depth (PRD 3 U3, U4, U8): item pairs per unit, teammates, the units that carry each trait
///    breakpoint. Written per scope by `tft-stats` (`unit_depth.rs`), kept out of [`STAT_TABLES`] as migration 4's are.
///    `item_c` '' means any third item (or none).
pub const MIGRATIONS: [(u32, &str); 7] = [
    (1, "CREATE TABLE IF NOT EXISTS stat_watermark (scope TEXT PRIMARY KEY, last_rowid INTEGER, built_at INTEGER)"),
    (
        2,
        "
CREATE TABLE IF NOT EXISTS rank_samples (match_id TEXT, puuid TEXT, tier TEXT, division TEXT, PRIMARY KEY (match_id, puuid));
CREATE TABLE IF NOT EXISTS rank_sample_queue (match_id TEXT PRIMARY KEY, platform TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS rank_sample_pending ON rank_sample_queue(done, platform);
",
    ),
    (3, "CREATE TABLE IF NOT EXISTS engine_log (at INTEGER, region TEXT, level TEXT, message TEXT)"),
    (
        4,
        "
CREATE TABLE IF NOT EXISTS stat_comp_level_bands (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, band INTEGER NOT NULL, level INTEGER NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL, rounds INTEGER NOT NULL, sum_round INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, band, level)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_carry_units (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, character_id TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL, full_games INTEGER NOT NULL, full_sum_place INTEGER NOT NULL,
  full_top4 INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, character_id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_carry_items (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, character_id TEXT NOT NULL, item TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, character_id, item)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_builds (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, character_id TEXT NOT NULL, build TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, character_id, build)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_holders (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, carry TEXT NOT NULL, holder TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL, top4 INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, carry, holder)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_special (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, item TEXT NOT NULL, holder TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, item, holder)
) WITHOUT ROWID;
",
    ),
    (
        5,
        "
CREATE TABLE IF NOT EXISTS comp_overrides (comp_id TEXT PRIMARY KEY, name TEXT, merge_into TEXT);
CREATE TABLE IF NOT EXISTS comp_lineage (patch TEXT NOT NULL, comp_id TEXT NOT NULL, prev_patch TEXT NOT NULL,
  prev_comp_id TEXT NOT NULL, overlap REAL NOT NULL, PRIMARY KEY (patch, comp_id));
",
    ),
    (
        6,
        "
CREATE TABLE IF NOT EXISTS stat_comp_flex_adds (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, character_id TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, character_id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_comp_swaps (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, comp_key TEXT NOT NULL, out_unit TEXT NOT NULL, in_unit TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, comp_key, out_unit, in_unit)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS comp_difficulty (comp_key TEXT PRIMARY KEY, level TEXT NOT NULL);
",
    ),
    (
        7,
        "
CREATE TABLE IF NOT EXISTS stat_unit_pairs (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, character_id TEXT NOT NULL, item_a TEXT NOT NULL, item_b TEXT NOT NULL,
  item_c TEXT NOT NULL, games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, character_id, item_a, item_b, item_c)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_teammates (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, character_id TEXT NOT NULL, other TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, character_id, other)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS stat_trait_sets (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, name TEXT NOT NULL, tier_current INTEGER NOT NULL, units TEXT NOT NULL,
  games INTEGER NOT NULL, sum_place INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket, name, tier_current, units)
) WITHOUT ROWID;
",
    ),
];

const RATES: &str = "games INTEGER NOT NULL, sum_place INTEGER NOT NULL, wins INTEGER NOT NULL, top4 INTEGER NOT NULL";

fn stat(name: &str, keys: &str, cols: &str) -> String {
    format!(
        "\nCREATE TABLE IF NOT EXISTS {name} (\n  patch TEXT NOT NULL, bucket TEXT NOT NULL, {cols},\n  PRIMARY KEY (patch, bucket, {keys})\n) WITHOUT ROWID;"
    )
}

/// Stat tables are rebuilt from scratch by the stats step, so they are safe to drop and recreate.
pub const STAT_TABLES: [&str; 13] = [
    "stat_meta", "stat_units", "stat_unit_dist", "stat_traits", "stat_items", "stat_unit_items", "stat_unit_builds",
    "stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels", "stat_comp_variants",
];

/// Bump when the stat table layout changes: old stat tables are dropped and rebuilt on the next stats run.
/// 3: stat_comps gained score, tier, prev_place and d3_place (PRD 2 C3, C4).
/// 4: the comp page tables (migration 4) are filled by a rebuild, so stats built before them are rebuilt in full.
/// 5: stat_comps gained core and flex, and stat_comp_variants was added (PRD 2 C1, C2).
/// 6: the flex and swap tables (migration 6) are filled by a rebuild, so stats built before them are rebuilt in full.
/// 7: the unit and trait page tables (migration 7) are filled by a rebuild (PRD 3 U3, U4, U8).
pub const STATS_VERSION: i64 = 7;

pub fn stats_schema() -> String {
    let r = RATES;
    format!(
        "
CREATE TABLE IF NOT EXISTS stat_meta (
  patch TEXT NOT NULL, bucket TEXT NOT NULL,
  matches INTEGER NOT NULL, comps INTEGER NOT NULL, built_at INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket)
) WITHOUT ROWID;
{}
{}
{}
{}
{}
CREATE INDEX IF NOT EXISTS stat_unit_items_item ON stat_unit_items(patch, bucket, item);
{}
{}
{}
CREATE INDEX IF NOT EXISTS stat_comp_units_unit ON stat_comp_units(patch, bucket, character_id);
{}
{}
{}
{}
",
        stat("stat_units", "character_id", &format!("character_id TEXT NOT NULL, {r}, avg_star REAL")),
        stat(
            "stat_unit_dist",
            "character_id, star, n_items",
            &format!("character_id TEXT NOT NULL, star INTEGER NOT NULL, n_items INTEGER NOT NULL, {r}"),
        ),
        stat(
            "stat_traits",
            "name, tier_current",
            &format!("name TEXT NOT NULL, tier_current INTEGER NOT NULL, num_units INTEGER NOT NULL, {r}"),
        ),
        stat("stat_items", "item", &format!("item TEXT NOT NULL, {r}, place_change REAL")),
        stat(
            "stat_unit_items",
            "character_id, item",
            &format!("character_id TEXT NOT NULL, item TEXT NOT NULL, {r}, place_change REAL"),
        ),
        stat(
            "stat_unit_builds",
            "character_id, build",
            &format!("character_id TEXT NOT NULL, build TEXT NOT NULL, {r}, place_change REAL"),
        ),
        // core and flex: JSON lists of unit ids, filled by the clusters comp method only (NULL under baseline).
        stat(
            "stat_comps",
            "comp_key",
            &format!("comp_key TEXT NOT NULL, {r}, sum_level INTEGER NOT NULL, score REAL, tier TEXT, prev_place REAL, d3_place REAL, core TEXT, flex TEXT"),
        ),
        stat(
            "stat_comp_units",
            "comp_key, character_id",
            "comp_key TEXT NOT NULL, character_id TEXT NOT NULL, games INTEGER NOT NULL, sum_star INTEGER NOT NULL, sum_items INTEGER NOT NULL, full_items INTEGER NOT NULL, sum_place INTEGER NOT NULL",
        ),
        stat(
            "stat_comp_items",
            "comp_key, character_id, item",
            "comp_key TEXT NOT NULL, character_id TEXT NOT NULL, item TEXT NOT NULL, games INTEGER NOT NULL, sum_place INTEGER NOT NULL",
        ),
        stat(
            "stat_comp_traits",
            "comp_key, name, tier_current",
            "comp_key TEXT NOT NULL, name TEXT NOT NULL, tier_current INTEGER NOT NULL, num_units INTEGER NOT NULL, style INTEGER NOT NULL, games INTEGER NOT NULL",
        ),
        stat(
            "stat_comp_levels",
            "comp_key, level",
            "comp_key TEXT NOT NULL, level INTEGER NOT NULL, games INTEGER NOT NULL, sum_place INTEGER NOT NULL",
        ),
        stat(
            "stat_comp_variants",
            "comp_key, variant_id",
            &format!("comp_key TEXT NOT NULL, variant_id TEXT NOT NULL, units TEXT NOT NULL, carries TEXT NOT NULL, {r}"),
        ),
    )
}
