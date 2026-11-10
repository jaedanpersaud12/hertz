# PRD 7: Live game capture

Weeks 9 to 11 (spike in week 5) · stories L1 to L14 · depends on PRD 6 (game detection, window tracking) · feeds PRD 8 (timeline, scouting) and PRD 9 (augment tiers).

The app records, from your own games, what Riot's API leaves out: augments, the board each round, HP, gold, level and positioning. This is the least certain PRD and is ordered so the first three stories are worth shipping alone.

## Context

Riot's match data is a photograph of the final board. Augments were removed from it in November 2024, and it never held rounds, economy or hex positions. MetaTFT has these because its overlay watches every user's game. We can watch one player.

That changes what the data is for. Global augment win rates need hundreds of thousands of games. A personal recorder needs twenty: enough to show you that you bleed HP in stage 3 or level a round late.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Augments | Offers and picks correct, on 20 hand-labelled games | 95% of picks, 90% of offers |
| Boards | Rounds with a captured end-of-planning board | 90% |
| Board accuracy | Units, stars and items correct on 100 labelled boards | 95% units, 90% stars, 85% items |
| HUD numbers | HP, gold, level correct per round | 98% |
| Cost | CPU averaged over a game; frame rate impact | Under 3%; within 1% |
| Resilience | Games where capture fails silently | 0 (it always reports its state) |

## Non-goals

Reading game memory or network traffic. Capturing opponents' boards beyond what you choose to scout on screen. Any action taken in the game. Global statistics from one player's data.

## Hard rules

1. Only two inputs: what the client publishes locally (API and log files) and pixels of the game window.
2. Capture is observation, the same as a recording of your own screen.
3. Before week 9, the week 1 policy read decides whether any captured value may be shown live in the overlay or only after the game. Default until then: recorded live, shown after.

## Sources

| Source | Expected to give | Confidence | Confirmed by |
| --- | --- | --- | --- |
| League client local API | Game phase, lobby members, queue, end-of-game | Medium | Spike L1 |
| In-game local API | Possibly limited or nothing for TFT | Low | Spike L1 |
| Game log files | Round transitions, possibly events | Unknown | Spike L1 |
| Screen: stage indicator | Stage and round | High | Template match |
| Screen: augment selection | Three offers, the pick | High, large static cards | Icon match plus OCR of the name |
| Screen: HUD | HP, gold, level, streak | High, fixed digits | Digit OCR |
| Screen: board | Units, stars, items, hexes | Medium, units animate and overlap | Icon match in hex regions |
| Screen: bench and shop | Bench units, shop offers | Medium | Later, not in this plan |

## Pipeline

1. **Trigger.** The stage tracker fires capture moments: augment screen visible, two seconds before planning ends, combat start.
2. **Grab.** Capture only the game window, only at trigger moments. No continuous recording.
3. **Locate.** Regions come from a per-set layout file expressed as fractions of the window, so any resolution works (L13).
4. **Recognise.** Icons are matched against Community Dragon art already cached, scaled to the region size. Numbers and names go through OCR restricted to the expected character set.
5. **Validate.** Every reading is checked against game rules: unit count within level, HP never rises without cause, gold within bounds, augment name exists in static data. Failures are stored as "unsure" with the image crop.
6. **Store.** Rows in the capture tables, keyed by a local game id.
7. **Join.** After the game, the Riot match id is attached. The final board from Riot corrects the last captured board, which also gives a running accuracy score for free.

## Data model

```
cap_games    (id, started_at, match_id NULL, set_number, layout_version, status)
cap_rounds   (game_id, stage, round, hp, gold, level, streak, phase_ended_at)
cap_boards   (game_id, stage, round, hex, unit, star, items TEXT, confidence)
cap_augments (game_id, slot, offered TEXT, picked, rerolled, confidence)
cap_unsure   (game_id, stage, round, region, crop BLOB, reason)
```

Export format (L12): one compressed JSON file per game containing these rows minus image crops, plus set number and app version.

## Insights built on it

| View | Shows | Story |
| --- | --- | --- |
| Match timeline | Scrubber by round: board, HP, gold, level, augments | L7 |
| My augments | Picks, avg place, pick rate when offered | L8 |
| My openers | Stage 2 and 3 boards grouped by trait, with final place | L9 |
| My tempo | Level-up rounds and gold curve, top 4 against bottom 4 | L10 |
| My positioning | Heatmap per comp of where carries stand, by result | L11 |

Every view states its sample size. Under 10 games it shows the games, not averages.

## Edge cases

| Case | Behaviour |
| --- | --- |
| Game UI patched, regions shift | Validation failure rate spikes; status turns "partial"; a calibration screen lets you nudge regions |
| New set | New layout file and icon set; capture disabled until the layout is verified |
| Overlay panel covers a capture region | Panels are excluded from the grab where the platform allows, otherwise hidden for the 50 ms of the grab |
| Alt-tabbed at a trigger moment | Round marked missed; no guess |
| Spectating or replay | Detected by game phase; capture off |
| Carousel and special rounds | Stage tracker knows them; board capture skipped |
| Unusual language or font | Names matched by icon first; OCR is the tie-breaker only |

## Test plan

- **Recorded games:** 5 full-game screen recordings become the regression suite. The pipeline runs on the video offline and its output is compared with hand labels.
- **Synthetic:** boards rendered from icons at 3 resolutions and 2 UI scales.
- **Live accuracy:** the Riot final board against the last captured board, tracked per game on the Data page.
- **Cost:** CPU and frame time with capture on and off.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| Spike (week 5) | Record every local API response and log line for one game; write up what is usable | L1 |
| 1 | Window grab, layout file, calibration screen | L13 |
| 2 | Stage tracker and triggers | L2 |
| 3 | Augment recognition; status dot | L3, L14 |
| 4 | HUD numbers; join to match | L5, L6 |
| 5 to 6 | Board recognition and validation | L4 |
| 7 | Timeline view | L7 |
| 8 | Augment, opener, tempo and positioning views | L8 to L11 |
| 9 | Export and import; labelled accuracy run | L12 |

## Open questions

- OCR engine: a bundled Rust-friendly engine or the operating system's built-in text recognition. Decide in session 3 by accuracy on augment names.
- Whether to keep image crops for unsure readings beyond 7 days (disk).
- Whether friends pooling captures is wanted at all. If not, L12 is the first story to cut.
