# PRD 8: Match history, profile and lobby scouting

> **Policy change, 10 Nov 2026:** in-game and loading-screen scouting (P9, P10, the Scouting panel) is dropped. Riot's TFT policy bans scouting during play. P9 is now a post-game lobby review; P10 is cancelled. See "Policy check" in 00-overview.md.

Weeks 7 to 8 · stories P1 to P12 · P1 to P7 and P11, P12 depend only on PRD 1 · P8 to P10 depend on PRD 6 (game detection, overlay).

Your profile shows how you play over a whole set, any player can be looked up, and when a game loads the overlay tells you who is in the lobby and what they tend to play.

## Context

Match history today is your last games with full lobbies and a summoner card. There is no long view, no other players, and nothing happens around a live game. The split in this PRD is deliberate: the profile half needs only the Riot API and can be built any time; the live half waits for the overlay.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Complete history | Your ranked games this set stored locally | 100% of what Riot returns |
| Fresh | Time from game end to it appearing with LP change | Under 2 minutes |
| Lookup | Any Riot ID to profile on screen, cold | Under 5 seconds |
| Scouting | Load-in to all 7 opponents summarised | Under 20 seconds |
| Budget | Crawler throughput lost to profile and scouting jobs | Under 5% per day |

## Non-goals

Public profile pages. Leaderboards. Tracking players who have not been looked up or followed.

## Requirements

### Profile (P1 to P4, P11, P12)

- **Backfill.** On first run, page through your match ids from set start and store every game, all queues, flagged by queue.
- **LP history.** After each ranked game, read the league entry and store tier, division, LP, wins, losses with a timestamp. The LP graph is drawn from these snapshots; games before the first snapshot show placement only.
- **Header.** Rank, LP, games, avg place, top 4 rate, win rate, last 20 placements as medals.
- **Breakdowns.** Tables by comp, unit (as carry) and trait: your games, avg place, top 4, and the difference from the global figure at your rank bucket. Sorting by that difference shows strengths and blind spots.
- **Distribution.** Histogram of placements; current and best top 4 streak.
- **Filters.** Patch, comp, placement range, queue. Ranked is the default everywhere.

### Lookup and friends (P5, P6)

- Search by Riot ID and region. Fetch account, league entry and the last 20 match ids; fetch only matches not already stored.
- Looked-up players are cached for 10 minutes. Followed players refresh every 30 minutes at low priority.
- A followed player's profile uses the same page as yours.

### Match detail (P7)

- All 8 boards with units, stars, items and traits, sorted by placement, each linking to that player.
- If a capture exists for the match, the timeline from PRD 7 appears here.

### Post-game summary (P8)

- Triggered by the `post game` state. Shows placement, LP change, your comp and its global avg place at your rank, and one notable line chosen by rule (for example: best placement with this comp, or a carry item that is far below average on that unit).
- Appears in the main window and as a notification. Never over the game.

### Lobby scouting (P9, P10)

1. At load-in, get the 8 lobby members from the client's local API (fallback: read names from the loading screen).
2. For each opponent: use stored matches first; fetch rank and the last 5 match ids only if stored data is older than a day.
3. Summarise: rank, last 10 placements, top 3 comps this patch with counts.
4. Compare with the pinned comp: if 2 or more opponents played it in their last 5 games, show the contested flag and the next-best comp sharing your opener units.
5. Panel shows for 20 seconds at load-in and is available on a hotkey afterwards.

## Request budget

| Job | Requests | When | Priority |
| --- | --- | --- | --- |
| Your refresh after a game | 2 to 3 | Each game | Highest |
| Scouting, warm (opponents already stored) | 0 to 7 | Each game | High |
| Scouting, cold | Up to 14 + 35 match fetches | Each game | High, spread over 40 seconds |
| Lookup | 3 + up to 20 | On demand | High |
| Followed players | 2 + new matches each | Every 30 minutes | Low |
| First backfill | 1 per 100 ids + 1 per match | Once | Low |

All of these share your home region's limit of 100 per 2 minutes with the crawler. The scheduler gives them priority and the crawler takes what is left.

## Data model

```
me_matches     (existing my_matches, widened: queue_id, comp_id, lp_after)
lp_snapshots   (puuid, at, tier, division, lp, wins, losses)
followed       (puuid, riot_id, platform, added_at, last_refresh)
lookups        (puuid, fetched_at)
scout_cache    (puuid, built_at, summary_json)
```

## Edge cases

| Case | Behaviour |
| --- | --- |
| Riot ID changed | Players are keyed by puuid; the displayed name updates on next refresh |
| Opponent with a private or brand-new account | Row shows rank only, "no recent games" |
| Placement games, no LP | Graph shows a gap, not zero |
| Demotion or promotion series | Tier bands on the graph; LP delta computed across tiers |
| Riot API down at game end | Summary shows placement from capture if available, fills in the rest when the API returns |
| Double Up partner | Shown as a pair in match detail; excluded from ranked stats |

## Policy check

Scouting opponents during a game is the feature most likely to be restricted by Riot's third-party rules. Until the week 1 policy read says otherwise: P9 and P10 are private-only, off by default, and would be removed from any released build.

## Test plan

- **Fixture:** a fake account with 300 games across two patches and a promotion; assert every profile number against a direct count.
- **Budget:** simulated day of 8 games with cold scouting; assert crawler throughput loss under 5%.
- **Timing:** scouting against the fake API with realistic latency completes in under 20 seconds.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Backfill, LP snapshots, priority jobs in the scheduler | P1, P2 |
| 2 | Profile header, breakdowns, distribution, filters | P3, P4, P11, P12 |
| 3 | Lookup and followed players | P5, P6 |
| 4 | Match detail rework | P7 |
| 5 | Post-game summary | P8 |
| 6 | Scouting job and panel; contested flag | P9, P10 |

## Open questions

- What the client's local API returns at load-in for TFT (from the L1 spike).
- Whether the notable line in the summary should be rule-based (planned) or model-written.
