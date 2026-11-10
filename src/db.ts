import Database from 'better-sqlite3'
import { mkdirSync } from 'node:fs'
import { dirname } from 'node:path'
import { config } from './config'

export type DB = Database.Database

const stat = (name: string, keys: string, cols: string) => `
CREATE TABLE IF NOT EXISTS ${name} (
  patch TEXT NOT NULL, bucket TEXT NOT NULL, ${cols},
  PRIMARY KEY (patch, bucket, ${keys})
) WITHOUT ROWID;`

const RATES = 'games INTEGER NOT NULL, sum_place INTEGER NOT NULL, wins INTEGER NOT NULL, top4 INTEGER NOT NULL'

/** Stat tables are rebuilt from scratch by "npm run stats", so they are safe to drop and recreate. */
export const STAT_TABLES = [
  'stat_meta', 'stat_units', 'stat_unit_dist', 'stat_traits', 'stat_items', 'stat_unit_items', 'stat_unit_builds',
  'stat_comps', 'stat_comp_units', 'stat_comp_items', 'stat_comp_traits', 'stat_comp_levels',
]

const SCHEMA = `
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
  comp_key   TEXT                          -- "main trait|carry unit", see src/cluster.ts
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

-- ---- Names, costs and icons from Community Dragon ("npm run static") -----------
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
`

const STATS_SCHEMA = `
CREATE TABLE IF NOT EXISTS stat_meta (
  patch TEXT NOT NULL, bucket TEXT NOT NULL,
  matches INTEGER NOT NULL, comps INTEGER NOT NULL, built_at INTEGER NOT NULL,
  PRIMARY KEY (patch, bucket)
) WITHOUT ROWID;
${stat('stat_units', 'character_id', `character_id TEXT NOT NULL, ${RATES}, avg_star REAL`)}
${stat('stat_unit_dist', 'character_id, star, n_items', `character_id TEXT NOT NULL, star INTEGER NOT NULL, n_items INTEGER NOT NULL, ${RATES}`)}
${stat('stat_traits', 'name, tier_current', `name TEXT NOT NULL, tier_current INTEGER NOT NULL, num_units INTEGER NOT NULL, ${RATES}`)}
${stat('stat_items', 'item', `item TEXT NOT NULL, ${RATES}, place_change REAL`)}
${stat('stat_unit_items', 'character_id, item', `character_id TEXT NOT NULL, item TEXT NOT NULL, ${RATES}, place_change REAL`)}
CREATE INDEX IF NOT EXISTS stat_unit_items_item ON stat_unit_items(patch, bucket, item);
${stat('stat_unit_builds', 'character_id, build', `character_id TEXT NOT NULL, build TEXT NOT NULL, ${RATES}, place_change REAL`)}
${stat('stat_comps', 'comp_key', `comp_key TEXT NOT NULL, ${RATES}, sum_level INTEGER NOT NULL`)}
${stat('stat_comp_units', 'comp_key, character_id', `comp_key TEXT NOT NULL, character_id TEXT NOT NULL, games INTEGER NOT NULL, sum_star INTEGER NOT NULL, sum_items INTEGER NOT NULL, full_items INTEGER NOT NULL, sum_place INTEGER NOT NULL`)}
CREATE INDEX IF NOT EXISTS stat_comp_units_unit ON stat_comp_units(patch, bucket, character_id);
${stat('stat_comp_items', 'comp_key, character_id, item', `comp_key TEXT NOT NULL, character_id TEXT NOT NULL, item TEXT NOT NULL, games INTEGER NOT NULL, sum_place INTEGER NOT NULL`)}
${stat('stat_comp_traits', 'comp_key, name, tier_current', `comp_key TEXT NOT NULL, name TEXT NOT NULL, tier_current INTEGER NOT NULL, num_units INTEGER NOT NULL, style INTEGER NOT NULL, games INTEGER NOT NULL`)}
${stat('stat_comp_levels', 'comp_key, level', `comp_key TEXT NOT NULL, level INTEGER NOT NULL, games INTEGER NOT NULL, sum_place INTEGER NOT NULL`)}
`

/** Bump when the stat table layout changes: old stat tables are dropped and rebuilt on the next "npm run stats". */
const STATS_VERSION = 2

function migrate(db: DB) {
  const has = (table: string, col: string) =>
    (db.prepare(`PRAGMA table_info(${table})`).all() as { name: string }[]).some((c) => c.name === col)
  if (!has('participants', 'comp_key')) db.exec('ALTER TABLE participants ADD COLUMN comp_key TEXT')
  if (!has('units', 'rarity')) db.exec('ALTER TABLE units ADD COLUMN rarity INTEGER')

  if (db.pragma('user_version', { simple: true }) !== STATS_VERSION) {
    for (const t of STAT_TABLES) db.exec(`DROP TABLE IF EXISTS ${t}`)
    db.pragma(`user_version = ${STATS_VERSION}`)
  }
}

export function openDb(path = config.dbPath, opts: { readonly?: boolean } = {}): DB {
  if (!opts.readonly) mkdirSync(dirname(path), { recursive: true })
  const db = new Database(path, { readonly: opts.readonly ?? false, fileMustExist: opts.readonly ?? false })
  db.pragma('busy_timeout = 10000')
  if (!opts.readonly) {
    db.pragma('journal_mode = WAL') // lets the app read while the crawler writes
    db.pragma('synchronous = NORMAL')
    db.exec(SCHEMA)
    migrate(db) // adds columns to databases made by an older version, drops outdated stat tables
    db.exec(STATS_SCHEMA)
  }
  return db
}

export const kvGet = <T,>(db: DB, key: string): T | null => {
  const row = db.prepare('SELECT value FROM kv WHERE key = ?').get(key) as { value: string } | undefined
  return row ? (JSON.parse(row.value) as T) : null
}
export const kvSet = (db: DB, key: string, value: unknown) =>
  db.prepare('INSERT INTO kv (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value').run(key, JSON.stringify(value))
