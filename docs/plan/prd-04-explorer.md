# PRD 4: Data Explorer

Week 4 · stories X1 to X8 · depends on PRD 1 · PRD 5 (builder live stats) and PRD 6 (overlay search) reuse its index.

Any question of the form "boards with these conditions, grouped by that" returns in under half a second, from an in-memory index built in Rust.

## Context

The current Explorer accepts up to four units and three traits and runs SQL joins per query. That is fine at 10,000 boards and will take seconds at 8 million. MetaTFT's explorer is its most powerful page and, by your account, one of its slowest. This is the place to be visibly better.

The index works like the tabs on a filing cabinet. Instead of reading every folder to find boards with a unit, each unit has a tab listing exactly which folders contain it. A query with three conditions compares three tabs, never the folders.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Speed | 95th percentile query time, 8 million boards | Under 500 ms; typical under 50 ms |
| Memory | Index for one patch scope | Under 1 GB, released after 5 idle minutes |
| Start-up | Time to first query after opening the Explorer | Under 1 second (memory-mapped file) |
| Expressiveness | The 12 reference questions below | All answerable |

## Non-goals

Free-text or natural-language queries. Cross-patch queries in one request (run it twice and compare).

## Query model

A query is a list of conditions joined by AND, plus a grouping.

| Condition | Fields | Example |
| --- | --- | --- |
| Unit | id, min star, max star, item count, up to 3 items, negate | 3-star X with item Y |
| Trait | id, min tier, negate | Trait Z at breakpoint 2 or higher |
| Item anywhere | id, negate | Any unit holding this artifact |
| Level | min, max | Level 9 or more |
| Placement | min, max | Top 4 only |
| Comp | comp id or variant id | Inside one comp |

| Grouping | Returns |
| --- | --- |
| Summary | Games, avg place, top 4, win rate, placement distribution |
| Units | Other units on matching boards, with place change against the summary |
| Items on unit N | Items and builds on one of the conditioned units |
| Traits | Active traits and tiers |
| Level | Results split by final level |

```
POST /api/explore
{ "patch":"16.22", "scope":"DIAMOND+", "where":[ {"unit":"TFT_X","minStar":3,"items":["TFT_Item_Y"]},
  {"trait":"TFT_Z","minTier":2,"not":true}, {"level":{"min":9}} ], "group":"units", "baseline":true }
```

## Index design

- **Rows.** One fixed-width record per board: placement, level, rank bucket, day, comp id, then offsets into unit and trait arrays.
- **Bitmaps.** One compressed bitmap per unit, per unit-at-star, per unit-with-item, per trait-at-tier, per item, per level and per placement. A condition is a bitmap; a query is their intersection.
- **Scopes.** Rank bucket and the 3-day window are two more bitmaps, so one index serves all 8 scopes of a patch.
- **Aggregation.** Grouping walks only the matching rows, in parallel across cores.
- **File.** Written by the stats step as one file per patch, memory-mapped on open, rebuilt incrementally by appending rows and extending bitmaps.

## Interface requirements

1. Conditions are chips in a row. Clicking a chip opens its options; a keyboard-only path exists for all of it.
2. Results update on every change with no submit button. Requests are cancelled when a newer one starts.
3. The URL holds the whole query, so back, forward and sharing work.
4. Grouping tabs switch without re-running the filter (the server caches the last matching set per session).
5. Baseline toggle (X8) shows each number's delta against the query minus its last condition.
6. "Explore this" on comp, unit, item and trait pages opens the Explorer pre-filled (X7).
7. A result with under 30 games shows the boards themselves instead of statistics.

## Reference questions

The 12 questions the finished Explorer must answer, used as the acceptance test:

1. How does a 3-star 1-cost carry place with and without a specific item?
2. What are the best items on unit A when unit B is also on the board?
3. Which units appear most on level 9 boards that finish first?
4. How does trait T at breakpoint 3 place without its usual 5-cost?
5. What is the best fourth unit for these three?
6. Do boards with two specific emblems outperform one?
7. Which artifacts are best on unit A at Master+?
8. How does comp C place when the lobby is Diamond+ in the last 3 days?
9. What level do winning boards of comp C end at?
10. Which traits are most often splashed alongside trait T?
11. How does unit A at 2 stars with 3 items compare to 3 stars with 2?
12. What do bottom-4 boards of comp C lack compared with top-4 ones?

## Edge cases

| Case | Behaviour |
| --- | --- |
| Contradictory conditions (unit and not the same unit) | Zero results with a line explaining which two conflict |
| Zero matches | Empty state suggests removing the most restrictive condition, named |
| Index file missing or from an older format | Rebuilt in the background; Explorer shows progress and falls back to SQL for small queries |
| Query while stats rebuild appends rows | Queries see the last complete snapshot |
| Low-memory machine | Setting to cap the index to the current patch and one rank scope |

## Test plan

- **Equivalence:** 500 random queries on the fixture return the same numbers from the index and from plain SQL.
- **Benchmark:** the 12 reference questions on the full database, timed in CI on your machine.
- **Fuzz:** malformed query bodies never crash the server.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Index writer in `tft-stats`, bitmap layout, memory-map reader | X5 |
| 2 | Query engine, summary and units grouping, equivalence test | X1, X4 |
| 3 | Remaining conditions and groupings, negation | X2, X3, X4 |
| 4 | UI: chips, URL state, cancellation | X1 to X4 |
| 5 | Saved queries, pre-fill links, baseline | X6, X7, X8 |

## Open questions

- OR between conditions (unit A or unit B) is left out. Worth adding if reference questions turn out to need it.
- Bitmap library choice (a roaring-bitmap crate is the planned starting point) is confirmed in session 1.
