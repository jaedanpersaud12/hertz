/**
 * Collects ranked TFT matches into the local database.
 *
 *   npm run crawl                     run until you press Ctrl+C
 *   npm run crawl -- --max 5000       stop after storing 5000 matches
 *   npm run crawl -- --reseed         refresh the player list from the ladders first
 *   npm run crawl -- --once           stop when every known player is up to date
 *
 * It is safe to stop and restart at any time: the queue and progress live in the database.
 */
import { config, PLATFORM_ROUTE, TIERS, tierRank } from '../config'
import { openDb } from '../db'
import { patchAt, refreshCalendar } from '../patches'
import { makeIngest } from './ingest'
import { getApexLeague, getLeaguePage, getMatch, getMatchIds, requestCounts, RiotKeyError, RiotUnavailableError } from './riot'

const args = process.argv.slice(2)
const flag = (name: string) => args.includes(`--${name}`)
const option = (name: string) => {
  const i = args.indexOf(`--${name}`)
  return i >= 0 ? args[i + 1] : undefined
}
const maxMatches = Number(option('max') ?? Infinity)
const RECRAWL_MS = 6 * 3600 * 1000 // look at the same player again after this long
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

const db = openDb()
const ingest = makeIngest(db)
let stored = 0
let stopping = false

const q = {
  playerCount: db.prepare('SELECT COUNT(*) n FROM players WHERE platform = ?'),
  upsertPlayer: db.prepare(`
    INSERT INTO players (puuid, platform, tier, rank) VALUES (?,?,?,?)
    ON CONFLICT(puuid) DO UPDATE SET tier = excluded.tier, rank = excluded.rank, platform = excluded.platform`),
  nextPlayers: db.prepare(`
    SELECT puuid, platform, tier, rank, last_crawled FROM players
    WHERE platform IN (SELECT value FROM json_each(?)) AND last_crawled < ?
    ORDER BY last_crawled ASC, rank DESC LIMIT ?`),
  markPlayer: db.prepare('UPDATE players SET last_crawled = ? WHERE puuid = ?'),
  enqueue: db.prepare('INSERT OR IGNORE INTO match_queue (match_id, route, platform, seed_rank) VALUES (?,?,?,?)'),
  pending: db.prepare('SELECT match_id, platform, seed_rank FROM match_queue WHERE route = ? AND status = 0 LIMIT ?'),
  pendingCount: db.prepare('SELECT COUNT(*) n FROM match_queue WHERE route = ? AND status = 0'),
  setStatus: db.prepare('UPDATE match_queue SET status = ? WHERE match_id = ?'),
  totals: db.prepare('SELECT (SELECT COUNT(*) FROM matches) matches, (SELECT COUNT(*) FROM players) players, (SELECT COUNT(*) FROM match_queue WHERE status = 0) pending'),
}

/** Fill the player list for one server from its ranked ladders. */
async function seed(platform: string) {
  const save = db.transaction((entries: { puuid?: string }[], tier: string) => {
    let n = 0
    for (const e of entries) {
      if (!e.puuid) continue
      q.upsertPlayer.run(e.puuid, platform, tier, tierRank(tier))
      n++
    }
    return n
  })

  for (const tier of config.tiers) {
    if (stopping) return
    let found = 0
    let missing = 0
    if (['CHALLENGER', 'GRANDMASTER', 'MASTER'].includes(tier)) {
      const entries = await getApexLeague(platform, tier)
      found = save(entries, tier)
      missing = entries.length - found
    } else if (TIERS.includes(tier as never)) {
      for (const division of ['I', 'II', 'III', 'IV']) {
        for (let page = 1; page <= config.pagesPerDivision && !stopping; page++) {
          const entries = await getLeaguePage(platform, tier, division, page)
          if (!entries.length) break
          const n = save(entries, tier)
          found += n
          missing += entries.length - n
        }
      }
    }
    console.log(`[${platform}] ${tier}: ${found} players` + (missing ? ` (${missing} entries had no puuid and were skipped)` : ''))
  }
}

/** One loop per routing group. Each group has its own rate limit, so they run side by side. */
async function crawlRoute(route: string, platforms: string[]) {
  const platformsJson = JSON.stringify(platforms)
  const sinceSec = () => Math.floor(Date.now() / 1000) - config.days * 86400

  let finished = false // set by --once when this region has nothing left to fetch
  while (!stopping && !finished && stored < maxMatches) {
    try {
      await step()
    } catch (err) {
      if (!(err instanceof RiotUnavailableError)) throw err
      // Nothing was marked as done, so the same players and matches are retried after the pause.
      console.log(`[${route}] ${err.message} Pausing this region for a minute.`)
      for (let i = 0; i < 60 && !stopping; i++) await sleep(1000)
    }
  }

  /** One round: top up the queue, then fetch a batch of matches. */
  async function step() {
    // Top up the queue by asking a few players for their recent matches.
    if ((q.pendingCount.get(route) as { n: number }).n < 40) {
      const players = q.nextPlayers.all(platformsJson, Date.now() - RECRAWL_MS, 8) as {
        puuid: string; platform: string; rank: number; last_crawled: number
      }[]
      if (!players.length && (q.pendingCount.get(route) as { n: number }).n === 0) {
        if (flag('once')) { finished = true; return }
        console.log(`[${route}] every known player is up to date. Checking again in 10 minutes.`)
        for (let i = 0; i < 600 && !stopping; i++) await sleep(1000)
        return
      }
      await Promise.all(players.map(async (p) => {
        const since = Math.max(sinceSec(), Math.floor(p.last_crawled / 1000))
        const ids = await getMatchIds(route, p.puuid, since)
        db.transaction(() => {
          for (const id of ids) q.enqueue.run(id, route, p.platform, p.rank)
          q.markPlayer.run(Date.now(), p.puuid)
        })()
      }))
    }

    const batch = q.pending.all(route, 20) as { match_id: string; platform: string; seed_rank: number }[]
    await Promise.all(batch.map(async (job) => {
      if (stopping || stored >= maxMatches) return
      const match = await getMatch(route, job.match_id)
      const queueId = match?.info.queue_id ?? match?.info.queueId
      if (!match || queueId !== config.rankedQueueId || match.info.participants.length < 2) {
        q.setStatus.run(2, job.match_id)
        return
      }
      if (ingest(match, job.platform, job.seed_rank, TIERS[job.seed_rank], config.snowball)) stored++
      q.setStatus.run(1, job.match_id)
    }))
  }
}

async function main() {
  const unknown = config.platforms.filter((p) => !PLATFORM_ROUTE[p])
  if (unknown.length) throw new Error(`Unknown server(s) in PLATFORMS: ${unknown.join(', ')}`)

  // Matches are dated to a patch by a calendar from Community Dragon. Refreshed now and every three hours.
  const gotCalendar = await refreshCalendar(db)
  console.log(gotCalendar ? `Current patch: ${patchAt(Date.now())}` : 'Could not download the patch calendar. Matches will be labelled when "npm run stats" next runs.')
  const calendarTimer = setInterval(() => void refreshCalendar(db), 3 * 3600_000)

  for (const platform of config.platforms) {
    const have = (q.playerCount.get(platform) as { n: number }).n
    if (have === 0 || flag('reseed')) {
      console.log(`[${platform}] reading ladders...`)
      await seed(platform)
    }
  }

  const routes = new Map<string, string[]>()
  for (const p of config.platforms) routes.set(PLATFORM_ROUTE[p], [...(routes.get(PLATFORM_ROUTE[p]) ?? []), p])

  const started = Date.now()
  const report = setInterval(() => {
    const t = q.totals.get() as { matches: number; players: number; pending: number }
    const perHour = Math.round((stored / (Date.now() - started)) * 3600_000)
    console.log(
      `stored ${stored} this run (${perHour}/hour) | database: ${t.matches} matches, ${t.players} players, ${t.pending} queued | ` +
        `requests ok ${requestCounts.ok}, rate-limited ${requestCounts.limited}, retried ${requestCounts.retried}`,
    )
  }, 30_000)

  console.log(`Crawling ${[...routes].map(([r, ps]) => `${r} (${ps.join(', ')})`).join(' | ')}. Press Ctrl+C to stop.`)
  try {
    await Promise.all([...routes].map(([route, platforms]) => crawlRoute(route, platforms)))
  } finally {
    clearInterval(report)
    clearInterval(calendarTimer)
  }
  const t = q.totals.get() as { matches: number; players: number; pending: number }
  console.log(`Stopped. Stored ${stored} matches this run. Database now holds ${t.matches} matches. Run "npm run stats" to refresh the app.`)
}

process.on('SIGINT', () => {
  if (stopping) process.exit(1)
  stopping = true
  console.log('\nFinishing requests in flight, then stopping...')
})

main()
  .catch((err) => {
    stopping = true
    console.error(err instanceof RiotKeyError ? `\n${err.message}` : err)
    process.exitCode = 1
  })
  .finally(() => db.close())
