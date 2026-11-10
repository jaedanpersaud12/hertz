/**
 * Turns stored matches into the stat tables the app reads.
 *
 *   npm run stats                 rebuild the two most recent patches
 *   npm run stats -- --patches 4  rebuild the four most recent patches
 */
import { fileURLToPath } from 'node:url'
import { BUCKETS, scopeKey, WINDOWS } from '../config'
import { openDb, STAT_TABLES, type DB } from '../db'
import { loadCalendar, refreshCalendar, relabelUnknown } from '../patches'

/** A comp needs this share of all boards (and at least this many) to get its own row. */
const COMP_MIN_SHARE = 0.002
const COMP_MIN_GAMES = 5
const COMP_LIMIT = 120

/**
 * The label from src/cluster.ts can split one real comp in two, for example when a comp has two carries and
 * either can end up holding more items. Here, labels whose usual boards are nearly the same get folded into
 * the more common one. Works on the temporary table p only; stored labels are untouched.
 */
function mergeLookalikes(db: DB, minGames: number) {
  const keys = db.prepare(`SELECT comp_key, COUNT(*) games FROM p WHERE comp_key IS NOT NULL
    GROUP BY comp_key HAVING COUNT(*) >= ? ORDER BY games DESC LIMIT 400`).all(Math.max(3, Math.floor(minGames / 4))) as { comp_key: string; games: number }[]
  if (keys.length < 2) return

  db.exec('CREATE TEMP TABLE IF NOT EXISTS cand (comp_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM cand')
  const add = db.prepare('INSERT INTO cand VALUES (?)')
  for (const key of keys) add.run(key.comp_key)
  const freq = db.prepare(`
    SELECT comp_key, character_id, COUNT(*) games
    FROM (SELECT DISTINCT p.id, p.comp_key, u.character_id FROM p JOIN cand USING (comp_key) JOIN units u ON u.participant_id = p.id)
    GROUP BY comp_key, character_id`).all() as { comp_key: string; character_id: string; games: number }[]

  const games = new Map(keys.map((key) => [key.comp_key, key.games]))
  const core = new Map<string, Set<string>>()
  for (const f of freq) {
    if (f.games / games.get(f.comp_key)! < 0.5) continue
    core.set(f.comp_key, (core.get(f.comp_key) ?? new Set()).add(f.character_id))
  }

  // Biggest first, so smaller look-alikes fold into the label most people ended up with.
  const kept: string[] = []
  const remap = db.prepare('UPDATE p SET comp_key = ? WHERE comp_key = ?')
  for (const { comp_key } of keys) {
    const mine = core.get(comp_key) ?? new Set<string>()
    const twin = kept.find((other) => {
      const theirs = core.get(other)!
      const shared = [...mine].filter((u) => theirs.has(u)).length
      const union = new Set([...mine, ...theirs]).size
      return union > 0 && shared / union >= 0.75
    })
    if (twin) remap.run(twin, comp_key)
    else if (mine.size) kept.push(comp_key)
  }
  db.exec('DROP TABLE IF EXISTS cand')
}

export function buildStats(db: DB, patchCount = 2, log: (s: string) => void = console.log) {
  const patches = (db.prepare(
    "SELECT patch FROM matches WHERE patch != 'unknown' GROUP BY patch ORDER BY MAX(game_datetime) DESC LIMIT ?",
  ).all(patchCount) as { patch: string }[]).map((r) => r.patch)

  if (!patches.length) {
    log('No matches stored yet. Run "npm run crawl" first.')
    return
  }

  const unlabelled = db.prepare('SELECT COUNT(*) n FROM participants WHERE comp_key IS NULL').get() as { n: number }
  const all = db.prepare('SELECT COUNT(*) n FROM participants').get() as { n: number }
  if (all.n && unlabelled.n > all.n * 0.5) log('Most boards have no comp label yet. Run "npm run recluster" once, then "npm run stats" again.')

  db.exec(`
    CREATE TEMP TABLE IF NOT EXISTS p (id INTEGER PRIMARY KEY, placement INTEGER NOT NULL, level INTEGER, comp_key TEXT);
    CREATE TEMP TABLE IF NOT EXISTS topc (comp_key TEXT PRIMARY KEY) WITHOUT ROWID;`)

  for (const patch of patches) {
    for (const bucket of BUCKETS) {
      for (const window of WINDOWS) {
        const scope = scopeKey(bucket.key, window.key)
        const since = window.days ? Date.now() - window.days * 86400_000 : 0
        const k = [patch, scope]
        const t0 = Date.now()

        db.transaction(() => {
          db.exec('DELETE FROM p; DELETE FROM topc')
          db.prepare(`
            INSERT INTO p SELECT pa.id, pa.placement, pa.level, pa.comp_key FROM participants pa
            JOIN matches m ON m.match_id = pa.match_id
            WHERE m.patch = ? AND m.rank >= ? AND m.game_datetime >= ?`).run(patch, bucket.min, since)

          for (const table of STAT_TABLES) db.prepare(`DELETE FROM ${table} WHERE patch = ? AND bucket = ?`).run(...k)

          const comps = (db.prepare('SELECT COUNT(*) n FROM p').get() as { n: number }).n
          const matches = (db.prepare('SELECT COUNT(*) n FROM matches WHERE patch = ? AND rank >= ? AND game_datetime >= ?')
            .get(patch, bucket.min, since) as { n: number }).n
          db.prepare('INSERT INTO stat_meta VALUES (?,?,?,?,?)').run(...k, matches, comps, Date.now())
          if (!comps) return

          // ---- Units ----
          // One row per unit, star level and item count. A board with two copies of a unit counts once.
          db.prepare(`
            INSERT INTO stat_unit_dist
            SELECT ?, ?, character_id, star, n_items, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4)
            FROM (SELECT u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, p.placement
                  FROM units u JOIN p ON p.id = u.participant_id GROUP BY u.participant_id, u.character_id)
            GROUP BY character_id, star, n_items`).run(...k)
          db.prepare(`
            INSERT INTO stat_units
            SELECT patch, bucket, character_id, SUM(games), SUM(sum_place), SUM(wins), SUM(top4), SUM(star * games) * 1.0 / SUM(games)
            FROM stat_unit_dist WHERE patch = ? AND bucket = ? GROUP BY character_id`).run(...k)

          // ---- Traits ---- per activation level, then one row (tier_current = 0) for "active at any level".
          db.prepare(`
            INSERT INTO stat_traits
            SELECT ?, ?, t.name, t.tier_current, MIN(t.num_units), COUNT(*), SUM(p.placement), SUM(p.placement = 1), SUM(p.placement <= 4)
            FROM traits t JOIN p ON p.id = t.participant_id GROUP BY t.name, t.tier_current`).run(...k)
          db.prepare(`
            INSERT INTO stat_traits
            SELECT patch, bucket, name, 0, MIN(num_units), SUM(games), SUM(sum_place), SUM(wins), SUM(top4)
            FROM stat_traits WHERE patch = ? AND bucket = ? GROUP BY name`).run(...k)

          // ---- Items ----
          db.prepare(`
            INSERT INTO stat_unit_items
            SELECT ?, ?, character_id, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), NULL
            FROM (SELECT DISTINCT ui.participant_id, ui.character_id, ui.item, p.placement
                  FROM unit_items ui JOIN p ON p.id = ui.participant_id)
            GROUP BY character_id, item`).run(...k)
          db.prepare(`
            INSERT INTO stat_items
            SELECT ?, ?, item, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), NULL
            FROM (SELECT DISTINCT ui.participant_id, ui.item, p.placement
                  FROM unit_items ui JOIN p ON p.id = ui.participant_id)
            GROUP BY item`).run(...k)
          db.prepare(`
            INSERT INTO stat_unit_builds
            SELECT ?, ?, u.character_id, u.build, COUNT(*), SUM(p.placement), SUM(p.placement = 1), SUM(p.placement <= 4), NULL
            FROM units u JOIN p ON p.id = u.participant_id WHERE u.build IS NOT NULL
            GROUP BY u.character_id, u.build`).run(...k)

          // Place change: how the unit places with this item (or build) compared with the unit overall.
          // Negative is better. This removes "good units make their items look good".
          for (const table of ['stat_unit_items', 'stat_unit_builds']) {
            db.prepare(`
              UPDATE ${table} SET place_change = sum_place * 1.0 / games - (
                SELECT su.sum_place * 1.0 / su.games FROM stat_units su
                WHERE su.patch = ${table}.patch AND su.bucket = ${table}.bucket AND su.character_id = ${table}.character_id)
              WHERE patch = ? AND bucket = ?`).run(...k)
          }
          // For an item overall: the average of its per-unit place changes, weighted by how often each unit holds it.
          db.prepare(`
            UPDATE stat_items SET place_change = (
              SELECT SUM(sui.place_change * sui.games) / SUM(sui.games) FROM stat_unit_items sui
              WHERE sui.patch = stat_items.patch AND sui.bucket = stat_items.bucket AND sui.item = stat_items.item)
            WHERE patch = ? AND bucket = ?`).run(...k)

          // ---- Comps ----
          const minGames = Math.max(COMP_MIN_GAMES, Math.ceil(comps * COMP_MIN_SHARE))
          mergeLookalikes(db, minGames)
          db.prepare(`
            INSERT INTO stat_comps
            SELECT ?, ?, comp_key, COUNT(*), SUM(placement), SUM(placement = 1), SUM(placement <= 4), SUM(COALESCE(level, 0))
            FROM p WHERE comp_key IS NOT NULL GROUP BY comp_key HAVING COUNT(*) >= ?
            ORDER BY COUNT(*) DESC LIMIT ?`).run(...k, minGames, COMP_LIMIT)
          db.prepare('INSERT INTO topc SELECT comp_key FROM stat_comps WHERE patch = ? AND bucket = ?').run(...k)

          db.prepare(`
            INSERT INTO stat_comp_units
            SELECT ?, ?, comp_key, character_id, COUNT(*), SUM(star), SUM(n_items), SUM(n_items = 3), SUM(placement)
            FROM (SELECT p.comp_key, u.character_id, MAX(u.star) star, MAX(u.n_items) n_items, p.placement
                  FROM p JOIN topc USING (comp_key) JOIN units u ON u.participant_id = p.id
                  GROUP BY p.id, u.character_id)
            GROUP BY comp_key, character_id`).run(...k)
          db.prepare(`
            INSERT INTO stat_comp_items
            SELECT ?, ?, comp_key, character_id, item, COUNT(*), SUM(placement)
            FROM (SELECT DISTINCT p.id, p.comp_key, ui.character_id, ui.item, p.placement
                  FROM p JOIN topc USING (comp_key) JOIN unit_items ui ON ui.participant_id = p.id)
            GROUP BY comp_key, character_id, item`).run(...k)
          db.prepare(`
            INSERT INTO stat_comp_traits
            SELECT ?, ?, p.comp_key, t.name, t.tier_current, MIN(t.num_units), MAX(t.style), COUNT(*)
            FROM p JOIN topc USING (comp_key) JOIN traits t ON t.participant_id = p.id
            GROUP BY p.comp_key, t.name, t.tier_current`).run(...k)
          db.prepare(`
            INSERT INTO stat_comp_levels
            SELECT ?, ?, p.comp_key, COALESCE(p.level, 0), COUNT(*), SUM(p.placement)
            FROM p JOIN topc USING (comp_key) GROUP BY p.comp_key, COALESCE(p.level, 0)`).run(...k)
        })()

        const meta = db.prepare('SELECT matches, comps FROM stat_meta WHERE patch = ? AND bucket = ?').get(...k) as { matches: number; comps: number }
        log(`patch ${patch}  ${scope.padEnd(13)} ${String(meta.matches).padStart(8)} matches ${String(meta.comps).padStart(9)} comps  ${((Date.now() - t0) / 1000).toFixed(1)} s`)
      }
    }
  }
  db.exec('DROP TABLE IF EXISTS p; DROP TABLE IF EXISTS topc')
  db.pragma('optimize')
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const i = process.argv.indexOf('--patches')
  const db = openDb()
  // Matches stored without a patch (Riot stopped sending one) get dated from the patch calendar first.
  const unknown = (db.prepare("SELECT COUNT(*) n FROM matches WHERE patch = 'unknown'").get() as { n: number }).n
  if (unknown) {
    if (!(await refreshCalendar(db))) loadCalendar(db)
    const fixed = relabelUnknown(db)
    console.log(fixed ? `Dated ${fixed.toLocaleString('en-US')} matches to their patch.` : 'Could not reach Community Dragon for the patch calendar. Check your connection and run this again.')
  }
  buildStats(db, i >= 0 ? Number(process.argv[i + 1]) : 2)
  db.close()
}
