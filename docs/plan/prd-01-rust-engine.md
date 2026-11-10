# PRD 1: Rust engine and data pipeline

Weeks 1 to 3 · stories E1 to E12 · depends on nothing · everything else depends on this.

The TypeScript engine is replaced by a Rust workspace that produces identical numbers, runs unattended, and holds 1 million matches per patch.

## Context

The current engine works and is tested against a fake Riot API. On real data it has stored about 1,300 matches at roughly 8,000 an hour with zero rate-limit hits. Its weaknesses are operational: four commands in order, a terminal that must stay open, a full stats recount every time, and a Node runtime that cannot live inside a desktop app cleanly.

Think of the port as moving house with an inventory list. The TypeScript engine is the list: every table and every API response must arrive unchanged before the old house is sold.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Same numbers | Parity diff on fixture and on a 50,000-match real sample | Empty (tolerance 0.0001 on merged comp averages) |
| Throughput | Requests used per region per 2-minute window over 24 hours | 95 of 100 or more, zero 429s |
| Freshness | Age of stats against newest stored match | Under 30 minutes |
| Volume | Matches for the current patch after 6 crawl days | 1 million |
| Footprint | Resident memory while crawling; database size | Under 80 MB; under 15 GB |

## Non-goals

Cloud hosting, production key, multi-user sync, changing any stat definition (that is PRD 2 and 3).

## Functional requirements

### Riot client (`tft-riot`)

1. One limiter per routing group (americas, europe, asia, sea), each with a short window (20 per second) and a long window (100 per 2 minutes).
2. A slot is released one full window after the response arrives, not after the request is sent. This is the rule that removed intermittent 429s in the current engine.
3. Limits are read from the `X-App-Rate-Limit` response header and adopted, so a different key type speeds up without code changes.
4. On 429: honour `Retry-After`, block that group, retry the same request.
5. On 401 or 403: stop all groups and surface a key error. On first start after a new key, retry for 2 minutes before reporting, because new keys take time to activate.
6. On 5xx or network failure: back off up to 6 tries, then pause that group for 60 seconds. The queued match stays queued.

### Crawl loop

1. Seed players from Challenger, Grandmaster and Master ladders plus paged lower tiers per platform.
2. Per group, loop: pick the least recently crawled player, fetch match ids since the patch start, queue unseen ids, fetch and store queued matches.
3. Snowball: participants of stored matches become players, inheriting the seed player's rank.
4. Skip non-ranked queues for stats, but store them flagged (E12).
5. All state is in the database. Killing the process at any point loses at most the in-flight request.

### Store and ingest (`tft-store`)

- Opens the existing database file unchanged. Migrations are numbered and idempotent.
- One transaction per match. `INSERT OR IGNORE` on match id makes ingest safe to repeat.
- Patch is derived from the Community Dragon calendar, since Riot's `game_version` no longer carries it.
- WAL mode, one writer connection, a pool of read-only connections for the API and overlay.

### Stats (`tft-stats`)

- Scopes: patch by rank bucket (All, Platinum+, Diamond+, Master+) by window (whole patch, last 3 days).
- Incremental: a watermark per scope records the last ingested match row. A rebuild adds only newer matches to running sums. The 3-day window is the exception and is recomputed from raw rows, because matches fall out of it.
- A full rebuild remains available and is what the parity test runs.

### Scheduler

A single supervisor task owns: crawl loops (always), stats tick (every 10 minutes if new matches), calendar and static refresh (every 3 hours), prune and compact (daily, only when idle), profile and scouting jobs (on demand, highest priority on their region).

## Data model changes

```
ALTER TABLE matches ADD COLUMN queue_id INTEGER;
CREATE TABLE stat_watermark (scope TEXT PRIMARY KEY, last_rowid INTEGER, built_at INTEGER);
CREATE TABLE rank_samples (match_id TEXT, puuid TEXT, tier TEXT, division TEXT, PRIMARY KEY (match_id, puuid));
CREATE TABLE engine_log (at INTEGER, region TEXT, level TEXT, message TEXT);
```

## Edge cases

| Case | Behaviour |
| --- | --- |
| Laptop sleeps mid-crawl | On wake, limiter windows reset; in-flight request retried |
| Patch day, calendar not yet updated | Matches dated to the previous patch, then relabelled when the calendar updates |
| Match with fewer than 8 participants or missing units | Stored; excluded from stats if any participant has no placement |
| Database locked by another process | Clear error naming the other process; no retry storm |
| Disk full | Crawl pauses, tray shows the reason, nothing is corrupted (WAL) |
| Clock skew on the machine | Windows use monotonic time, not wall time |

## Test plan

- **Unit:** limiter under simulated time; patch dating; ingest of 20 saved real match files.
- **Parity:** fixture through both engines; diff every stat table and every API route.
- **Soak:** 24 hours against the real API; assert zero 429s and no memory growth.
- **Chaos:** the fake Riot API injects 429s, 500s, timeouts and malformed JSON; kill the process at random points; assert no duplicates and no skipped matches.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Workspace, `tft-core` types, serde models from saved match files |  |
| 2 | `tft-store`: open existing database, ingest, parity on raw tables | E1 |
| 3 | `tft-riot` limiter and client, simulated-time tests | E2, E4 |
| 4 | Crawl loop and resume | E3 |
| 5 to 6 | `tft-stats` full rebuild, parity on stat tables | E1 |
| 7 | `tft-api` routes, golden responses | E1 |
| 8 | Incremental stats and scheduler | E5, E6 |
| 9 | CLI, prune, settings, health view | E8 to E11 |
| 10 | Rank sampling, queue flag | E7, E12 |

## Open questions

- Does the 3-day window need to be exact, or is a daily bucket good enough? Daily buckets would make it incremental too.
- Is 1% rank sampling affordable? It costs 8 requests per sampled match, so about 8% of the request budget.
