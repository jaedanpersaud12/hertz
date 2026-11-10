import 'dotenv/config'
import { existsSync, readFileSync } from 'node:fs'

/**
 * dotenv treats everything after "#" as a comment, so RIOT_ID=name#tag arrives as just "name".
 * Read the line from .env ourselves when the tag is missing, so the unquoted form works too.
 */
function riotIdFromEnv(): string {
  const parsed = (process.env.RIOT_ID ?? '').trim()
  if (parsed.includes('#') || !existsSync('.env')) return parsed
  const line = readFileSync('.env', 'utf8').match(/^\s*RIOT_ID\s*=\s*(.*?)\s*$/m)
  const raw = (line?.[1] ?? '').replace(/^(["'])(.*)\1$/, '$2').trim()
  return raw.includes('#') ? raw : parsed
}

export const TIERS = [
  'IRON', 'BRONZE', 'SILVER', 'GOLD', 'PLATINUM', 'EMERALD', 'DIAMOND', 'MASTER', 'GRANDMASTER', 'CHALLENGER',
] as const
export type Tier = (typeof TIERS)[number]
export const tierRank = (tier: string) => Math.max(0, TIERS.indexOf(tier.toUpperCase() as Tier))

/** Which regional host serves match data for each platform. */
export const PLATFORM_ROUTE: Record<string, string> = {
  na1: 'americas', br1: 'americas', la1: 'americas', la2: 'americas',
  euw1: 'europe', eun1: 'europe', tr1: 'europe', ru: 'europe', me1: 'europe',
  kr: 'asia', jp1: 'asia',
  oc1: 'sea', sg2: 'sea', tw2: 'sea', vn2: 'sea',
}

/** Cumulative rank filters, the same idea as "Platinum+" in MetaTFT. */
export const BUCKETS = [
  { key: 'ALL', min: 0 },
  { key: 'PLATINUM+', min: tierRank('PLATINUM') },
  { key: 'DIAMOND+', min: tierRank('DIAMOND') },
  { key: 'MASTER+', min: tierRank('MASTER') },
] as const

/** Time windows inside a patch. '' is the whole patch. */
export const WINDOWS = [
  { key: '', label: 'Whole patch', days: null },
  { key: '3d', label: 'Last 3 days', days: 3 },
] as const

/** Stats are stored per patch and per scope, where a scope is a rank filter plus a time window: "PLATINUM+@3d". */
export const scopeKey = (bucket: string, window: string) => (window ? `${bucket}@${window}` : bucket)

const list = (v: string | undefined, fallback: string) =>
  (v ?? fallback).split(',').map((s) => s.trim()).filter(Boolean)

export const config = {
  apiKey: process.env.RIOT_API_KEY ?? '',
  /** `{host}` is replaced with a platform (na1) or route (americas). Overridden in tests. */
  apiBase: process.env.RIOT_API_BASE ?? 'https://{host}.api.riotgames.com',
  platforms: list(process.env.PLATFORMS, 'na1,euw1,kr,vn2').map((p) => p.toLowerCase()),
  tiers: list(process.env.TIERS, 'CHALLENGER,GRANDMASTER,MASTER,DIAMOND,EMERALD,PLATINUM').map((t) => t.toUpperCase()),
  pagesPerDivision: Number(process.env.PAGES_PER_DIVISION ?? 5),
  days: Number(process.env.DAYS ?? 7),
  snowball: (process.env.SNOWBALL ?? 'true') !== 'false',
  rankedQueueId: Number(process.env.RANKED_QUEUE_ID ?? 1100),
  dbPath: process.env.DB_PATH ?? 'data/tft.db',
  port: Number(process.env.PORT ?? 5174),
  /** Your own account, for the summoner card and match history: "Name#TAG" and the server you play on. */
  riotId: riotIdFromEnv(),
  myPlatform: (process.env.MY_PLATFORM ?? '').toLowerCase(),
  /** Fraction of the published rate limit we allow ourselves. */
  rateMargin: Number(process.env.RATE_MARGIN ?? 0.95),
}
