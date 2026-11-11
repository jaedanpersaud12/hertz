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
  rank          INTEGER NOT NULL          -- rank of the player this match was found through
);
CREATE INDEX IF NOT EXISTS matches_patch ON matches(patch, rank);

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
/// 1. Real ranks for a sample of lobbies (E7). `rank_samples` holds one row per looked-up player: `tier` is
///    Riot's tier, or `UNRANKED` when the player has no ranked TFT entry; `division` is I to IV (empty for apex
///    tiers' single division as Riot sends it, and for unranked players). `rank_sample_queue` holds every match
///    chosen for sampling; `done` turns 1 once all its players are in `rank_samples`. Rows are never removed, so
///    choosing a match again after a restart is a no-op.
pub const MIGRATIONS: [(u32, &str); 1] = [(
    1,
    "
CREATE TABLE IF NOT EXISTS rank_samples (match_id TEXT, puuid TEXT, tier TEXT, division TEXT, PRIMARY KEY (match_id, puuid));
CREATE TABLE IF NOT EXISTS rank_sample_queue (match_id TEXT PRIMARY KEY, platform TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS rank_sample_pending ON rank_sample_queue(done, platform);
",
)];

const RATES: &str = "games INTEGER NOT NULL, sum_place INTEGER NOT NULL, wins INTEGER NOT NULL, top4 INTEGER NOT NULL";

fn stat(name: &str, keys: &str, cols: &str) -> String {
    format!(
        "\nCREATE TABLE IF NOT EXISTS {name} (\n  patch TEXT NOT NULL, bucket TEXT NOT NULL, {cols},\n  PRIMARY KEY (patch, bucket, {keys})\n) WITHOUT ROWID;"
    )
}

/// Stat tables are rebuilt from scratch by the stats step, so they are safe to drop and recreate.
pub const STAT_TABLES: [&str; 12] = [
    "stat_meta", "stat_units", "stat_unit_dist", "stat_traits", "stat_items", "stat_unit_items", "stat_unit_builds",
    "stat_comps", "stat_comp_units", "stat_comp_items", "stat_comp_traits", "stat_comp_levels",
];

/// Bump when the stat table layout changes: old stat tables are dropped and rebuilt on the next stats run.
pub const STATS_VERSION: i64 = 2;

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
        stat("stat_comps", "comp_key", &format!("comp_key TEXT NOT NULL, {r}, sum_level INTEGER NOT NULL")),
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
    )
}
