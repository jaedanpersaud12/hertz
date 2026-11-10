# TFT Local: 3-Month Product Plan

Nov 10, 2026 · @Jaedan Persaud

## Summary

In 12 weeks TFT Local goes from a stats website on localhost to a desktop app with an in-game overlay and its own live-game data. The plan is ten PRDs in three phases, 115 user stories, each sized for one AI build session.

Think of it as building a restaurant. Today we have a kitchen (the crawler and stats) and a dining room (nine pages). Phase 1 stocks the pantry properly, Phase 2 adds table service (the overlay), Phase 3 grows our own vegetables (data Riot does not sell).

|  | Today (v10) | After week 12 |
| --- | --- | --- |
| Data | About 8,000 matches an hour while the crawler is open in a terminal | Background service, 1 million+ matches per patch, stats rebuilt automatically |
| Comps | Trait + carry labels, one board per comp | Variants, levelling guide, early boards, item priority, positioning, trends |
| Stats depth | Units, items, traits, simple explorer | Builds by star and item count, item pairs, trait breakpoints, explorer with any filter |
| App | Browser tab, four terminal commands | Installed desktop app, one click, auto-updating data |
| In game | Nothing | Overlay: pinned comp, item cheat sheet, lobby scouting, augment picker |
| Own data | None | Augments, stage-by-stage boards and positioning captured from your own games |
| Content | None | Your notes and guides on any comp, imported pro comps |

Three things decide how close to MetaTFT this gets, and all three are in the Ground rules section: what Riot's API does not contain, what one player's captures can and cannot replace, and what Riot's policies allow an overlay to show.

**Stack decision:** the engine (crawler, ingest, stats, local API, live capture) is rewritten in Rust and runs as one binary inside a Tauri desktop app. The React interface stays as it is.

**Tracking:** every story is an issue in the [TFT Local project in Linear](https://linear.app/jaedan/project/tft-local-0a9f37cff9ef) (JAE-184 to JAE-302), one milestone per PRD, due on the Sunday of its planned week.

## Ground rules

MetaTFT's edge is not its website, it is thousands of overlay users feeding it data Riot's API never returns. Every feature below is sorted by where its data can come from, because that decides when it can be built.

| Data | Riot API | Our own capture | Consequence |
| --- | --- | --- | --- |
| Final boards, items, traits, placements | Yes | Not needed | Phase 1. Scale is the only limit |
| Rank of each lobby | Approximate (rank of the seed player) | Not needed | Phase 1: look up real ranks for a sample to check the approximation |
| Augments | No, removed November 2024 | Yes, from your own games | Phase 3. Stats are personal-sized, so tiers start from an imported list |
| Round-by-round boards, early comps, HP, levelling timings | No | Yes, from your own games | Phase 3. A levelling guide can be estimated earlier from final level and last round |
| Hex positioning | No | Yes, from your own games | Phase 3. Until then positioning is something you save by hand in the builder |
| Pro comps, guides, difficulty tags | No | No, written by people | Phase 3 as your own notes plus import |

### What one player's captures can do

One player produces roughly 5 to 10 games a day. That is enough for a personal review tool (what you picked, what it cost you) and far too little for global augment or positioning statistics. The plan treats capture as a personal dataset and designs an export format, so friends running the app can pool captures later.

### Policy limits to respect

- **Key type.** A personal key is for private use. If this is ever shared beyond you and friends it needs a production key and Riot's review.
- **No memory reading.** The game runs under Vanguard. Capture uses only what Riot exposes (the local client API and log files) plus reading the screen, the same way a stream viewer would.
- **Overlay content.** Riot has restricted what third-party tools may show during a game, augment win rates in particular. Checked on 10 Nov 2026; the verdict for each story is in the policy check below.
- **Mac versus Windows.** You play and build on a Mac; the linked PC is Windows. Overlays over the game work on both only in borderless mode. Week 5 starts with a one-day spike on the machine you actually play on.

### Policy check, 10 Nov 2026

Riot's rule for TFT tools comes down to one line: static, pre-game information is fine during a game; anything that reacts to what is happening in this game, or to the people in this lobby, is not. Riot also blocked game memory access for unknown third-party apps from 6 Oct 2026, which our design never used.

Think of it like an exam with notes allowed. You can bring your own notes in (comp guides, tier lists, item charts). You cannot have someone whisper answers based on what is on your paper, or tell you what the student next to you is writing.

| Story | Verdict | Why |
| --- | --- | --- |
| D4 pinned comp, D5 comp list, D6 item sheet, D7 lookup | Allowed | Static data available before the game |
| B9 level boards in the overlay | Allowed if you step them yourself | Auto-advancing on your detected level would be a real-time recommendation |
| G4 augment tier list in game | Allowed as a list you search | Auto-highlighting the three augments on offer would react to the game |
| G4, L8 augment win rates | Allowed, except Legends and Legend-based augments | Riot bans those win rates in any app |
| L1 to L7, L13, L14 capture of your own game | Allowed as recording; results shown after the game | Post-game analysis is what Riot encourages; capture uses screen and client API, not memory |
| L9 to L11 personal insights | Allowed | Post-game review |
| P1 to P8, P11, P12 profile, lookup, match detail, post-game card | Allowed | Outside the game or after it |
| P9 lobby scouting at load-in | Dropped | Riot bans scouting during play, including the loading screen and lobby-wide stats |
| P10 contested warning | Dropped | Same rule; it is lobby scouting |

P9 and P10 are replaced by one post-game story: the lobby you just played, with what each opponent ran and how they usually play, shown after the game ends. Sources: [Riot TFT game policy](https://developer.riotgames.com/docs/tft), [Third Party Applications policy, 10 Feb 2025](https://support.riotgames.com/en-us/riot/events/third-party-applications), [game memory access removed, effective 6 Oct 2026](https://support.riotgames.com/en-us/riot/performance/game-memory-access-removed-for-third-party-apps).

## How we build: Rust engine, AI sessions

The engine becomes one Rust workspace that Tauri loads directly, so the finished app is a single installed program with no Node, no terminal and no second process.

### Why Rust is worth the rewrite here

- **One binary.** Tauri's backend is Rust. With a Node engine the desktop app would have to bundle and babysit a Node process; with Rust the engine is just a library the app calls.
- **Runs beside the game.** A crawler that sits in the tray while you play should use tens of megabytes, not hundreds.
- **Capture needs it.** Screen reading and log tailing in Phase 3 are native work either way.
- **What it will not do:** make the stats queries much faster. SQLite does that work in both versions. The speed win is in ingest and in clustering, which move from row-at-a-time JavaScript to in-memory Rust.

The cost is about one and a half weeks of Phase 1. The existing TypeScript engine is the specification: same database schema, same API routes, same numbers.

### Workspace layout

| Crate | Job | Main libraries |
| --- | --- | --- |
| `tft-core` | Types, comp labelling, patch calendar, tier cut-offs | serde, time |
| `tft-riot` | Riot client, per-region rate limiter, retries | reqwest, tokio |
| `tft-store` | SQLite schema, migrations, ingest | rusqlite (bundled) |
| `tft-stats` | Aggregation, lookalike merge, scopes | rusqlite, rayon |
| `tft-api` | The `/api/*` routes the React app already calls | axum |
| `tft-capture` | Client API, log tailing, screen reading (Phase 3) | notify, xcap, an OCR crate chosen in a spike |
| `tft-cli` | `crawl`, `stats`, `static`, `serve` for running without the app | clap |
| `app` | Tauri shell, tray, overlay windows | tauri 2 |

Library names are the planned starting point; each is confirmed against current docs in the session that first uses it.

### The parity harness

The safety net for the port is a golden test: load the same fixture matches into the TypeScript engine and the Rust engine, then compare every stats table and every API response byte for byte. The existing end-to-end test and its fake Riot API become that fixture. A crate is done when the diff is empty.

### How a session runs

1. Pick one user story. Paste its row and the PRD's data notes into the session.
2. AI writes the test first (parity, unit or a screenshot check), then the code.
3. Gate: `cargo test`, `cargo clippy` clean, parity diff empty, and for UI stories the stress-data switch (worst case, empty, 1,000 rows) plus a screenshot review.
4. You play-test for five minutes and either accept or write one line on what is off.
5. Commit with the story id. One story, one commit.

A realistic pace is two to four stories on a weekday evening and six to eight on a free day. The plan has 115 stories, so it needs about 10 a week. That is tight but workable, and the cut order in the last section says what goes first if it slips.

### Definition of done

- [ ] Acceptance criteria in the story all pass
- [ ] Works on real data, not only the fixture
- [ ] Empty, loading and error states exist and say what to do next
- [ ] No new terminal step for the user
- [ ] README section updated if a number's meaning changed

## Roadmap

Three four-week phases, each ending in a gate that must pass before the next phase starts.

&#91;embedded content: roadmap · 10 PRDs, 3 phases, 3 gates\]

PRDs overlap on purpose: the comps work starts as soon as the Rust store can read the existing database, and match history is built while the overlay is being hardened.

| Phase | Weeks | Ships | Gate to pass |
| --- | --- | --- | --- |
| 1 Data parity | 1 to 4 (16 Nov to 13 Dec) | Rust engine, 1 million matches, new comps engine, deeper stats, Explorer | Parity diff empty, TypeScript engine retired, 90% of top 4 boards in a named comp |
| 2 App and overlay | 5 to 8 (14 Dec to 10 Jan) | Installed app, tray engine, overlay panels, Team Planner export, profile and scouting | 10 real games played with the overlay: zero crashes, zero focus steals, no terminal used |
| 3 Own data and polish | 9 to 12 (11 Jan to 7 Feb) | Capture, match timelines, augment browser and tiers, notes, onboarding, finished identity | Augments recorded in 95% of games; every success measure in the last section met or consciously cut |

Week 1 also holds the three checks everything else leans on: the Riot policy read, the set schedule, and the parity harness. Phase 2 falls over the holidays, so it carries the fewest must-have stories per week.

## PRD 1: Rust engine and data pipeline at scale

**Weeks 1 to 3.** The engine is ported to Rust with identical output, then made to run unattended and hold a million matches per patch.

**Problem.** Collecting data takes four terminal commands in the right order, stops when the terminal closes, and has only been proven on about 1,300 real matches. Stats are rebuilt from scratch each time, which will take minutes at scale.

**Goals**

- Rust engine passes the parity harness against the TypeScript engine on every table and route.
- 1 million matches for the current patch within 6 days of crawling (4 regions at about 8,000 an hour).
- Stats are never more than 30 minutes behind the crawler, with no manual step.
- Database stays under 15 GB by pruning patches older than the last three.

**Not in scope.** A cloud server, a production API key, sharing data between users.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| E1 | the Rust engine to give the same numbers as today's | nothing I trust changes under me | Parity diff is empty for all stat tables and all `/api` routes on the fixture |
| E2 | the crawler to keep each region at its limit without tripping it | I get the most data my key allows | 24-hour run: zero 429s, at least 95 of 100 slots used per 2 minutes per region |
| E3 | crawling to resume exactly where it stopped | closing the laptop loses nothing | Kill the process mid-match; restart stores no duplicates and skips nothing |
| E4 | a clear message when my key is missing, expired or not yet active | I do not debug a 401 again | Each case shows its own sentence and the fix; activation delay is retried for 2 minutes first |
| E5 | stats to update on their own as matches arrive | the app is always current | Incremental rebuild touches only scopes with new matches; under 60 seconds for 50,000 new matches |
| E6 | new patches to be detected and set data refreshed automatically | patch day needs nothing from me | On a new Community Dragon patch folder: static data, icons and calendar update; old patch stays selectable |
| E7 | real ranks checked for a sample of lobbies | I know how far off the rank filter is | 1% of matches get all 8 ranks looked up; Data page shows the share landing in the right bucket |
| E8 | old patches pruned and the file compacted | the disk does not fill up | Raw rows older than 3 patches are deleted, their stat tables kept; file shrinks after compaction |
| E9 | to choose regions and tiers in the app | I can favour my own server or high elo | Settings change takes effect on the next crawl loop without a restart |
| E10 | a health view of the pipeline | I can see it working at a glance | Data page shows matches per hour per region, queue depth, last error, stats freshness |
| E11 | the engine to run without the app from a command line | I can leave it on another machine | `tft crawl`, `tft stats`, `tft serve` work from the single binary |
| E12 | Double Up, Hyper Roll and set revivals stored but kept apart | my ranked stats stay clean and other modes are there later | Queue id on every match; every stat scope filters to ranked by default |

**Data and design notes**

- Keep the current SQLite schema so the existing database opens in the Rust engine untouched.
- Incremental stats: keep running sums and counts per scope (games, sum of placement, wins, top 4) so adding matches is addition, not a recount. Comp merging still runs in full, but only on the comp table, which is small.
- Rate limiter keeps the tested rule: a slot is held until one full window after the response arrives.

**Risks.** Parity on comp merging is the hard part because floating-point order can differ; compare with a tolerance of 0.0001 there and exact elsewhere.

## PRD 2: Comps engine and tier list

**Weeks 2 to 4.** Comps stop being a label and become a full page: the board, its variants, how to level, what to build and how it is trending.

**Problem.** A comp today is "biggest trait plus main carry" with one typical board. MetaTFT's comp page answers five questions ours does not: which version of the comp, when to level, which items in what order, what to play before it comes online, and whether it is getting better or worse.

**Goals**

- At least 90% of boards that finish top 4 land in a named comp (measure today's figure in week 2 as the baseline).
- No two listed comps share more than 75% of core units.
- Comp names read like a player would say them, checked by hand on the top 30.

**Not in scope.** Real early-game boards and positioning from capture (PRD 7), written guides (PRD 9).

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| C1 | comps grouped by the units they share, not one trait and one carry | flex versions are not split or wrongly merged | Clustering on unit sets weighted by cost and items; goals above met on real data |
| C2 | variants inside a comp | I see the 4-cost flex or the alternate carry | Each comp lists up to 4 variants with their own avg place and pick rate, minimum 50 games |
| C3 | a tier letter that weighs placement and popularity | S tier is not a 20-game fluke | Score uses avg place, top 4 and a pick-rate confidence term; cut-offs documented in the README |
| C4 | the trend against last patch and the last 3 days | I catch what is rising before the lobby does | Arrow and delta on each row; a filter for "rising" |
| C5 | a levelling guide per comp | I know when to roll | Shows final level split and most common level at last round per placement band, labelled as estimated |
| C6 | item priority per carry | I slam the right components | Top 3 builds per carry with place change, plus best single items ranked by place change |
| C7 | the best units to hold items early | I do not sell my items with the holder | Lower-cost units sharing a trait or item class with the carry, ranked by appearance in winning boards |
| C8 | units that fit in or swap out | I adapt when I do not hit | "Add at level 9" and "replace X with Y" lists, each with the place change |
| C9 | emblem, artifact and radiant item value for the comp | I know which one to take | Table of special items with holder and place change, minimum 30 games |
| C10 | playstyle and difficulty tags | I filter to what I can pilot | Reroll, Fast 8, Fast 9 from data; difficulty stays a tag I set myself |
| C11 | to filter the tier list by unit, trait, item or playstyle | I answer "what uses this" in one step | Filters combine; URL holds the state |
| C12 | to compare two comps side by side | I pick between my two openers | Two columns: board, stats, items, trend |
| C13 | a comp to keep its identity across patches | favourites and notes survive patch day | Comp ids are matched to last patch by unit overlap; favourites carry over |
| C14 | to rename or merge comps myself | the list matches how I think | Rename and "merge into" stored locally and reapplied after every rebuild |

**Data and design notes**

- Clustering approach to test first: represent each board as its units weighted by cost and item count, group with a density method per patch, then name each group from its top trait and its three-item units. Keep today's rule as the fallback and the comparison.
- Levelling data is limited to final level and last round. That gives "most 1st places end at level 9", not "level to 8 on 4-2". Real timings arrive with capture.

**Risks.** Clustering quality is a judgement call. Budget two sessions for looking at real output and tuning before building the page on top.

## PRD 3: Units, items, traits and tables

**Weeks 3 to 4.** Every stat MetaTFT shows from final boards, we show, and every table answers follow-up questions without leaving the page.

**Problem.** Unit, item and trait pages cover the headline numbers. Missing are the cuts players use to make decisions: by star level and item count together, item pairs, trait breakpoints with specific units, and table tools such as column choice and export.

**Goals.** A unit page answers "what do I build on this, at this star level, in this comp" in under 3 clicks. Any table with 1,000 rows scrolls at 60 frames a second.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| U1 | unit stats split by star level and item count together | I know if 2-star with 3 items beats 3-star with 1 | Grid of star by items with avg place and games |
| U2 | best builds per unit filtered by comp | the build fits the board I am on | Build list takes a comp filter; falls back to all boards under 50 games |
| U3 | item pairs on a unit | I pick the third item given two I have | Choose up to 2 items; the list shows the best completions |
| U4 | which units a unit wins with | I find its natural partners | Top 10 teammates by place change, minimum 200 shared boards |
| U5 | component stats | I choose the right carousel item | Per component: the completed items it builds into, weighted by their value on current top comps |
| U6 | item stats by type with holders | I value artifacts, radiants and emblems properly | Type tabs; each item lists its best 5 holders with place change |
| U7 | emblem pages | I know who to put a spatula item on | Per emblem: best holders and the breakpoint it usually unlocks |
| U8 | trait pages per breakpoint with the usual units | I see what "6 of X" actually looks like | Each breakpoint: avg place, pick rate, most common unit set |
| U9 | every table to have column choice, sticky header and saved sort | I set it up once | Preferences persist per table |
| U10 | long tables to stay fast | 1,000 rows do not stutter | Rows are virtualised; the 1,000-row stress switch scrolls smoothly |
| U11 | to export any table as CSV | I can poke at it elsewhere | Export matches the visible columns and filters |
| U12 | a sample-size warning on thin numbers | I do not trust 12 games | Rows under the minimum-games setting are dimmed with the game count shown |
| U13 | patch notes next to the numbers | I link a buff to a jump | Each unit, item and trait page shows its lines from the current patch notes when available |

**Data notes.** U3 and U4 need two new stat tables (unit item pairs, unit pairs). Both are large; store only rows above the minimum games. U13 depends on finding a machine-readable patch notes source, which is an open question.

## PRD 4: Data Explorer

**Week 4.** The Explorer answers any question of the form "boards with these things, grouped by that" in under half a second.

**Problem.** Today it takes up to four units and three traits. MetaTFT's explorer filters on almost anything (unit with star level and specific items, trait at a breakpoint, level, placement range, excluded units) and groups the results by units, items, traits or level.

**Goals.** Any filter combination returns in under 500 ms on 8 million boards. Every query has a shareable URL.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| X1 | to filter by unit with star level, item count and specific items | I ask "3-star X holding Y" | Each unit condition takes star, item count and up to 3 items |
| X2 | to exclude units, traits and items | I ask "this comp without Z" | Any condition can be flipped to "not" |
| X3 | to filter by trait breakpoint, level and placement range | I look only at winning level 9 boards | Three more condition types, combinable |
| X4 | to group results by units, items on a unit, traits or level | one query answers "what else" and "what items" | Tabs change the grouping without rerunning the filter |
| X5 | results in under half a second | exploring feels like thinking | Measured on the full database; slow queries are logged on the Data page |
| X6 | to save and name queries | my regular questions are one click | Saved queries live in My lists |
| X7 | to open the Explorer pre-filled from any comp, unit or item | I dig deeper from where I am | "Explore this" on every detail page |
| X8 | to compare a query against its baseline | I see the effect of one change | Result shows delta against the same query minus the last condition |

**Data and design notes.** This is where Rust pays. Load one patch's boards into memory as compact arrays with a bitmap index per unit, trait and item: a filter is then a handful of bitmap intersections, and 8 million boards fit in well under 1 GB. The index is built by the stats step and memory-mapped so start-up stays instant.

**Risks.** Memory while the game is running. Load the index only while the Explorer is open and drop it after 5 idle minutes.

## PRD 5: Team Builder and Team Planner export

**Weeks 5 to 6.** The builder becomes the place you prepare a game: build a board, see how real boards like it place, and send it to the in-game Team Planner.

**Problem.** The builder counts traits and saves teams. It does not tell you whether the board is any good, and getting it into the game means rebuilding it by hand in the client.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| B1 | live stats for the board I am building | I know if my idea is good before I queue | As units change, the panel shows avg place, top 4 and games for boards containing them (Explorer index) |
| B2 | suggestions for the next unit | I finish a board fast | Top 5 additions by place change given the current units, respecting level |
| B3 | items on units with recipe help | the build is part of the plan | Drag items to units; components needed are totalled under the board |
| B4 | emblems and other trait modifiers counted | trait counts are right | An emblem on a unit adds its trait; a +1 trait toggle covers set mechanics |
| B5 | to copy a Team Planner code | the comp is in my client in one paste | Copy button produces a code the client accepts; verified by pasting into the live client |
| B6 | to import a Team Planner code | I load a comp a friend sent | Pasting a code fills the board |
| B7 | to open any comp from the tier list in the builder | I start from the meta and tweak | "Open in builder" on every comp and variant |
| B8 | positioning saved with the team | I remember my setup | Hex positions persist; teams show as boards in My lists |
| B9 | a level slider showing the board at 6, 7, 8 and 9 | I plan the whole game | Each level stores its own unit set; the overlay can step through them |
| B10 | to share a team as an image or link | I show friends | Image export of the board; the link opens the builder with the same state |

**Data notes.** B5 and B6 depend on the Team Planner code format for the current set. Start the PRD with a one-session spike: export a code from the client, decode it, confirm the unit index table can be derived from Community Dragon data. If the format cannot be derived each set, B5 and B6 fall back to a maintained lookup file.

**Not in scope.** Simulating fights or rating positioning.

## PRD 6: Desktop app and in-game overlay

**Weeks 5 to 8.** TFT Local becomes an installed app that starts with the computer, collects data from the tray and draws a fast, stable overlay over the game. This is the feature you started the project for.

**Problem.** MetaTFT's overlay is buggy and slow. Ours has to be the opposite: open in under 100 ms, never steal focus, never cost frames, and work with no network because the data is already on disk.

**Goals**

- Overlay panel opens in under 100 ms from hotkey press.
- No measurable frame-rate drop in game with the overlay visible (under 1% CPU while idle).
- Zero terminal commands from install to first comp list.

**Not in scope.** Reading game memory, clicking or acting in the game, exclusive-fullscreen support (borderless only).

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| D1 | one installer | setup is double-click | Signed build for the OS I play on; first run asks for the API key and Riot ID, then starts crawling |
| D2 | the engine in the tray | data collects while I do other things | Tray menu: status, pause, open, quit; survives sleep and wake |
| D3 | the app to notice when a TFT game starts and ends | the overlay appears only in game | Detected from the client's local API or the game process; overlay shows within 5 seconds of loading in |
| D4 | a pinned comp panel | my plan is always visible | Shows board, item priority and levelling notes for the pinned comp; steps through the level 6 to 9 boards |
| D5 | a comp tier list panel on a hotkey | I pivot without alt-tabbing | Hold or toggle hotkey; filter by the units I type |
| D6 | an item cheat sheet | I stop guessing recipes | Component grid; hovering a component highlights what it builds and the best holders in my pinned comp |
| D7 | unit and trait lookup on a hotkey | I check a build mid-round | Search box; result card with best builds |
| D8 | panels I can move, resize, fade and lock | it fits my screen | Positions persist per resolution; opacity per panel; click-through when locked |
| D9 | the overlay never to take focus or block clicks | it cannot lose me a round | Automated test: 1,000 show and hide cycles with the game focused, focus never changes |
| D10 | a compact mode for small screens | it works on a laptop | Under 1,440 px wide, panels use the compact layout |
| D11 | the overlay to stay correct if the engine is busy | crawling never lags it | Overlay reads from a read-only database handle; verified while a stats rebuild runs |
| D12 | to turn any panel off | I keep only what I use | Per-panel switches in Settings; choices marked private-only by the policy check stay off by default |
| D13 | the app to update itself | I get fixes without reinstalling | Tauri updater from a private release feed |

**Design notes**

- Each panel is its own transparent, always-on-top, non-activating Tauri window rendering the same React components as the main app.
- Game detection order: the League client's local API first, process name second.
- The overlay follows the app's motion rules: hotkey panels appear instantly, no entrance animation.

**Risks.** Transparent click-through windows behave differently on macOS and Windows, and Vanguard may object to overlays it does not recognise. The week 5 spike draws one empty window over a live game on the real machine before anything else is built. If that fails, the fallback is a second-screen or phone companion view fed by the same API.

## PRD 7: Live game capture

**Weeks 9 to 11.** The app records what the Riot API leaves out, from your own games: augments, the board at every stage, HP, gold, level and positioning.

**Problem.** Augment stats, early comps, stage timings and positioning are MetaTFT's moat, and all of it comes from its overlay watching games. We can watch only your games, so this PRD builds a personal game recorder first and a pooling format second.

**How it reads the game.** Like a referee watching from the stands: it looks at the screen and at what the client announces, and never reaches onto the pitch. Two sources, in order of trust:

1. The client's local API and log files, for game start, end and whatever round state they expose.
2. Screen reading at fixed moments (augment choice, end of planning phase): crop known regions, match unit and item icons against Community Dragon art, read numbers with OCR.

**Goals**

- Augment choices recorded correctly in 95% of games, checked against 20 hand-labelled games.
- End-of-planning board captured for at least 90% of rounds.
- Capture uses under 3% CPU averaged over a game.

**Not in scope.** Reading memory, capturing other players' boards beyond what scouting shows on screen, global augment statistics.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| L1 | a spike that proves what the client exposes | we build on facts | One recorded game with every local API response and log line saved; a written list of usable fields |
| L2 | round and stage tracked live | everything else has a clock | Stage shown in the overlay matches the game within 2 seconds for a full match |
| L3 | my augment offers and picks recorded | I get augment stats at all | Offered and chosen augments stored per game; accuracy goal met |
| L4 | my board saved at the end of each planning phase | I can replay how the game went | Units, stars, items and hex positions per round, stored against the match id |
| L5 | HP, gold, level and streak per round | I see where games are lost | Read from the HUD; shown as a timeline on the match page |
| L6 | captures joined to the Riot match afterwards | timeline and final result are one record | Join by match id within 5 minutes of game end |
| L7 | a timeline on each of my matches | I review a game in a minute | Scrubber by round: board, HP, gold, level, augments |
| L8 | personal augment stats | I learn which ones work for me | Per augment: picks, avg place, pick rate when offered; sample size always visible |
| L9 | my early-game patterns | I fix my openers | Stage 2 and 3 boards grouped by trait; avg final place per opener |
| L10 | my levelling and econ timings against my results | I see if I roll too early | Level-up round and gold at each stage, split by top 4 and bottom 4 |
| L11 | positioning heatmaps for my comps | I see my habits | Per comp: where each carry usually stands, split by result |
| L12 | capture files I can export and import | friends can pool data | One documented file per game; importing a friend's file adds to pooled stats, kept separate from mine |
| L13 | capture to survive resolution and UI scale changes | it keeps working | Regions are relative to the game window; a calibration screen confirms them; tested at three resolutions |
| L14 | a capture health indicator | I know when it is blind | Overlay dot: reading, partial or off, with the reason |

**Data notes.** New tables: `cap_games`, `cap_rounds`, `cap_boards`, `cap_augments`. Icon matching uses the art already cached for the UI. Set changes move HUD elements, so region definitions live in a data file per set, not in code.

**Risks.** This is the least certain PRD. Screen reading breaks with every UI change, and Riot's rules on capture tools must be checked in week 1. Stories are ordered so that L1 to L3 alone are worth shipping.

## PRD 8: Match history, profile and lobby scouting

**Weeks 7 to 8.** Your profile shows how you actually play, any player can be looked up, and when a game loads the overlay tells you who is in the lobby and what they like to play.

**Problem.** Match history lists your last games and a summoner card. There is no long-term picture (LP over time, best comps, weak spots), no lookup of other players, and no scouting.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| P1 | my full ranked history for the set stored locally | trends cover more than 20 games | Backfill on first run; new games added within 2 minutes of finishing |
| P2 | an LP and rank graph | I see my climb | LP recorded after every game from the league endpoint; line by day with tier bands |
| P3 | my stats by comp, unit and trait | I know what I am good at | Tables of my games: plays, avg place, top 4, compared with the global figure for my rank |
| P4 | placement distribution and streaks | I see consistency | Histogram of 1st to 8th; current and best streaks |
| P5 | to look up any player by Riot ID | I check a friend or a streamer | Search fetches rank and last 20 games on demand and caches them |
| P6 | a friends list | I follow people I play with | Saved players refresh in the background at low priority, behind the crawler's budget |
| P7 | each match to open into the full lobby | I study what beat me | All 8 boards with items and traits; links to each player's profile |
| P8 | a post-game summary when a game ends | I learn while it is fresh | Card: placement, LP change, my comp's global avg place against my result, one stat that stood out |
| P9 | lobby scouting when a game loads | I know who contests me | For each opponent: rank, last 10 placements, top 3 comps this patch; ready within 20 seconds of loading in |
| P10 | a contested warning for my pinned comp | I pivot early | Flag when 2 or more opponents played my pinned comp in their last 5 games |
| P11 | filters on my history | I review one comp or one patch | Filter by patch, comp, placement range, queue |
| P12 | Double Up and other modes shown but separated | history is complete without polluting ranked stats | Queue filter; ranked is the default |

**Data notes**

- P9 costs about 16 requests per lobby (7 opponents, rank plus match ids each) and up to 70 more for match details, on your own region's limit. The scouting job takes priority over the crawler for that minute, and uses matches already in the database first.
- Getting the lobby list needs the client's local API at load-in. Confirm in the L1 spike what it returns for TFT. If it returns nothing, scouting falls back to reading names from the loading screen.

**Policy check.** Lobby scouting during a game is the kind of feature Riot looks at closely. It is marked private-only until the week 1 policy read says otherwise.

## PRD 9: Curated content

**Weeks 10 to 11.** Anything that is opinion rather than data (guides, augment tiers, pro comps) gets a home, written by you or imported, and clearly labelled as such.

**Problem.** Part of MetaTFT is hand-written: pro comps, augment tier lists, how-to-play notes. No dataset produces these. The honest version is a notes layer over the data plus imports you choose.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| G1 | notes on any comp, unit, item or augment | my own lessons sit next to the numbers | Markdown note per entity; shown in the app and the overlay's pinned panel |
| G2 | a guide template per comp | notes have a shape | Sections: early game, levelling, items, positioning, when to pivot; all optional |
| G3 | an augment browser | I can read and search augments at all | Names, text and icons from Community Dragon, by tier (silver, gold, prismatic) |
| G4 | my own augment tier list | I have something to pick from in game | Drag augments into S to D; personal capture stats shown beside each when they exist |
| G5 | to import a tier list or comp list from a file | I use lists others publish | Documented JSON format; import shows a preview and the source name before saving |
| G6 | an AI draft of a comp guide from the data | I start from something | Button generates a draft from the comp's stats using my own model key; saved only when I accept; labelled as generated |
| G7 | pro and high-elo boards for a comp | I see how the best play it | Recent top 4 boards of the comp from Challenger players in the database, with player and region |
| G8 | follow specific players' boards | I track who I learn from | A followed player's recent comps appear on a Pros tab (uses P6) |
| G9 | a patch summary | I know what changed in two minutes | Biggest movers in units, items and comps since last patch, generated from the stats |

**Not in scope.** Copying another site's guides or tier lists into the app automatically. Imports are files you choose to bring in.

**Data notes.** G7 and G9 need no new data, only queries. G3 depends on the augment entries in the Community Dragon set file, which the static step already downloads but does not store.

## PRD 10: Identity, onboarding, settings and quality

**Week 12, with small pieces every phase.** The app looks like one product from installer to overlay, sets itself up, and stays fast.

| ID | As a player, I want | So that | Done when |
| --- | --- | --- | --- |
| Q1 | a first-run flow | I am looking at comps within minutes | Three steps: key, Riot ID, regions; a progress view while the first 5,000 matches arrive; demo data offered meanwhile |
| Q2 | one Settings page | nothing needs a config file | Key, Riot ID, regions, tiers, minimum games, hotkeys, overlay panels, storage limit; `.env` no longer needed |
| Q3 | the art identity finished | it feels like its own thing | Tier badges, placement medals, trait gems, empty states, app icon and tray icon in the cut-stone style; Quiver or Higgsfield art slotted in where supplied |
| Q4 | real-data polish | names, icons and ability text read right | Checked by hand on the current set: no raw ids, no broken icons, ability text free of markup |
| Q5 | every page under 100 ms | it never feels like MetaTFT | Route timings logged in dev; the 10 slowest listed on the Data page; all under budget on 1 million matches |
| Q6 | keyboard control everywhere | I move fast | Command palette reaches every page, comp, unit and item; documented shortcuts for filters |
| Q7 | a light theme and a compact density | it suits day and small screens | Theme and density tokens; contrast checked |
| Q8 | backup and restore | a reinstall loses nothing | Export of favourites, notes, teams, captures and settings to one file; raw matches excluded |
| Q9 | errors that explain themselves | I can fix or report them | One error surface with what happened, what to do, and a copyable log path |
| Q10 | the test suite to guard all of it | AI sessions do not break old features | Rust tests for engine, golden API tests, and screenshot tests of 12 key screens run on every commit |

**Notes.** Q4 is waiting on screenshots of the app on your real data. Q3 needs either the Quiver and Higgsfield tools connected or you running the prompts in `identity/BRIEF.md`.

## Risks, open decisions and success measures

The plan is aggressive: 115 stories at about 10 a week. The biggest threats are the three things we do not control, which is why each has a spike before any build work depends on it.

### Risks

| Risk | Hits | Early test | Fallback |
| --- | --- | --- | --- |
| Riot policy forbids an overlay feature | PRD 6, 7, 8 | Week 1: read the current policy, mark every overlay story | Feature stays private-only or moves to the post-game view |
| Overlay window does not draw over the game on your machine | PRD 6 | Week 5, day 1 spike | Second-screen or phone companion view |
| Client exposes too little for capture | PRD 7, P9 | L1 spike, pulled forward to week 5 | Screen reading only; scouting from the loading screen |
| Screen reading is unreliable | PRD 7 | Accuracy check on 20 labelled games | Ship L1 to L3 only (augments), drop boards and positioning |
| Rust port takes longer than 1.5 weeks | Everything | Parity harness by end of week 1 | Keep the TypeScript engine as a sidecar for the lagging crate |
| New clustering is worse than the simple rule | PRD 2 | Side-by-side on real data in week 2 | Keep today's rule, add variants on top |
| A new set lands mid-plan | All data | Check Riot's set schedule in week 1 | Pause features for 3 days: static data, HUD regions, Team Planner table |
| Too many stories | Schedule | Count done stories every Sunday | Cut order below |

### Cut order if behind

1. PRD 9 stories G6 to G9
2. PRD 7 stories L9 to L12
3. PRD 3 stories U11 to U13
4. PRD 5 stories B9 and B10
5. PRD 10 story Q7

### Decisions needed from you

- [ ] Decided 10 Nov: Windows. The overlay spike, installer and capture all target Windows first; the Mac stays a development machine for the engine and UI.
- [ ] Decided 10 Nov: private for now. Personal key, no Riot review, scouting and capture stay private-only features. Revisit after week 12.
- [ ] Rust for the whole engine from week 1 (this plan), or keep TypeScript running until the Rust port passes parity? The plan assumes the second: nothing is switched off until the diff is empty.
- [ ] Are you happy to use your own model key for the AI guide drafts in G6?

### Success measures at week 12

| Measure | Target |
| --- | --- |
| Matches stored for the current patch | 1 million or more |
| Any page or overlay panel | Under 100 ms |
| Explorer query | Under 500 ms |
| Top 4 boards landing in a named comp | 90% or more |
| Terminal commands needed in normal use | 0 |
| Overlay crashes or focus steals in 50 games | 0 |
| Augment capture accuracy | 95% or more |
| Your own verdict | You have uninstalled MetaTFT |
