import { config } from '../config'

/** Thrown when the key is missing, expired or not allowed. The crawler stops on this. */
export class RiotKeyError extends Error {}
/** Thrown when Riot keeps failing or cannot be reached. Nothing is lost: the caller waits and tries again. */
export class RiotUnavailableError extends Error {}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/**
 * Sliding-window rate limiter for one Riot host.
 * Riot enforces limits per host (na1, americas, ...), so each host gets its own limiter.
 *
 * Riot counts a request when it arrives, which is some time between when we send it and when the
 * answer comes back. So a request holds its slot from the moment it is sent until one full window
 * after its answer arrives. That way no window on Riot's side can ever hold more than we allow.
 */
export class Limiter {
  private slots: { done: number }[] = [] // done = Infinity while the request is in flight
  private blockedUntil = 0
  private chain: Promise<unknown> = Promise.resolve()
  private windows: { limit: number; ms: number }[]

  constructor(windows: [number, number][] = [[20, 1], [100, 120]], private margin = config.rateMargin) {
    this.windows = this.toWindows(windows)
  }

  private toWindows(w: [number, number][]) {
    return w.map(([count, seconds]) => ({
      limit: Math.max(1, Math.floor(count * this.margin)),
      ms: seconds * 1000,
    }))
  }

  /** Riot reports the real limits in a header like "20:1,100:120". Adopt them, so a bigger key just works. */
  updateFromHeader(header: string | null) {
    if (!header) return
    const parsed = header.split(',').map((p) => p.split(':').map(Number)).filter((p) => p.length === 2 && p.every(Number.isFinite))
    if (parsed.length) this.windows = this.toWindows(parsed as [number, number][])
  }

  /** Stop all requests on this host for a while (used after a 429). */
  block(ms: number) {
    this.blockedUntil = Math.max(this.blockedUntil, Date.now() + ms)
  }

  /** Resolves when it is safe to send one request. Call the returned function when the answer arrives. */
  acquire(): Promise<() => void> {
    const turn = this.chain.then(() => this.waitTurn())
    this.chain = turn.catch(() => {})
    return turn
  }

  private async waitTurn(): Promise<() => void> {
    for (;;) {
      const now = Date.now()
      if (now < this.blockedUntil) {
        await sleep(this.blockedUntil - now)
        continue
      }
      const longest = Math.max(...this.windows.map((w) => w.ms))
      this.slots = this.slots.filter((s) => s.done > now - longest)

      let wait = 0
      for (const w of this.windows) {
        const held = this.slots.filter((s) => s.done > now - w.ms)
        if (held.length < w.limit) continue
        // We need this many slots to free up before one more request fits.
        const need = held.length - w.limit + 1
        const finished = held.map((s) => s.done).filter(Number.isFinite).sort((a, b) => a - b)
        wait = Math.max(wait, finished.length >= need ? finished[need - 1] + w.ms - now + 5 : 50)
      }
      if (wait <= 0) {
        const slot = { done: Infinity }
        this.slots.push(slot)
        return () => { slot.done = Date.now() }
      }
      await sleep(wait)
    }
  }
}

const limiters = new Map<string, Limiter>()
const limiterFor = (host: string) => {
  let l = limiters.get(host)
  if (!l) limiters.set(host, (l = new Limiter()))
  return l
}

export const requestCounts = { ok: 0, retried: 0, limited: 0 }

/** GET a Riot endpoint. Returns null on 404. Retries on 429 and server errors. */
export async function riotGet<T>(host: string, path: string): Promise<T | null> {
  if (!config.apiKey) throw new RiotKeyError('RIOT_API_KEY is not set. Copy .env.example to .env and add your key.')
  const url = config.apiBase.replace('{host}', host) + path
  const limiter = limiterFor(host)

  for (let attempt = 0; attempt < 6; attempt++) {
    const answered = await limiter.acquire()
    let res: Response
    try {
      res = await fetch(url, { headers: { 'X-Riot-Token': config.apiKey }, signal: AbortSignal.timeout(20_000) })
      answered()
    } catch {
      answered()
      requestCounts.retried++
      await sleep(1000 * 2 ** attempt)
      continue
    }
    limiter.updateFromHeader(res.headers.get('x-app-rate-limit'))

    if (res.ok) {
      requestCounts.ok++
      return (await res.json()) as T
    }
    await res.body?.cancel()
    if (res.status === 404) return null
    if (res.status === 401 || res.status === 403) {
      throw new RiotKeyError(
        `Riot rejected the API key (${res.status}) on ${host}. Development keys expire every 24 hours: ` +
          'generate a new one at developer.riotgames.com and update .env.',
      )
    }
    if (res.status === 429) {
      requestCounts.limited++
      const retryAfter = Number(res.headers.get('retry-after'))
      limiter.block((Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter : 5) * 1000 + 250)
      continue
    }
    requestCounts.retried++
    await sleep(1000 * 2 ** attempt)
  }
  throw new RiotUnavailableError(`Riot did not answer on ${host} after several tries. Check your connection, or try again in a few minutes.`)
}

// ---- Endpoints -------------------------------------------------------------

export type LeagueEntry = { puuid?: string; leaguePoints?: number; tier?: string }

/** Challenger, Grandmaster and Master come back as one list each. */
export async function getApexLeague(platform: string, tier: string): Promise<LeagueEntry[]> {
  const res = await riotGet<{ entries?: LeagueEntry[] }>(platform, `/tft/league/v1/${tier.toLowerCase()}?queue=RANKED_TFT`)
  return res?.entries ?? []
}

/** Diamond and below are paged per division. An empty page means the division is exhausted. */
export async function getLeaguePage(platform: string, tier: string, division: string, page: number): Promise<LeagueEntry[]> {
  const res = await riotGet<LeagueEntry[]>(platform, `/tft/league/v1/entries/${tier}/${division}?queue=RANKED_TFT&page=${page}`)
  return res ?? []
}

export async function getMatchIds(route: string, puuid: string, startTimeSec: number | null, count = 30): Promise<string[]> {
  const since = startTimeSec ? `&startTime=${startTimeSec}` : ''
  const res = await riotGet<string[]>(route, `/tft/match/v1/matches/by-puuid/${puuid}/ids?start=0&count=${count}${since}`)
  return res ?? []
}

export type RiotMatch = {
  metadata: { match_id: string; participants: string[] }
  info: {
    game_datetime: number
    game_length: number
    game_version: string
    queue_id?: number
    queueId?: number
    tft_set_number?: number
    participants: {
      puuid: string
      riotIdGameName?: string
      riotIdTagline?: string
      placement: number
      level: number
      last_round: number
      gold_left: number
      total_damage_to_players: number
      traits: { name: string; num_units: number; style: number; tier_current: number }[]
      units: { character_id: string; tier: number; rarity?: number; itemNames?: string[] }[]
    }[]
  }
}

export const getMatch = (route: string, matchId: string) => riotGet<RiotMatch>(route, `/tft/match/v1/matches/${matchId}`)

// ---- Your own account (used by the app, not the crawler) -------------------

export type RiotAccount = { puuid: string; gameName: string; tagLine: string }
export const getAccount = (route: string, name: string, tag: string) =>
  riotGet<RiotAccount>(route, `/riot/account/v1/accounts/by-riot-id/${encodeURIComponent(name)}/${encodeURIComponent(tag)}`)

export type RankEntry = { queueType: string; tier: string; rank: string; leaguePoints: number; wins: number; losses: number }
export const getRank = async (platform: string, puuid: string) =>
  (await riotGet<RankEntry[]>(platform, `/tft/league/v1/by-puuid/${puuid}`)) ?? []
