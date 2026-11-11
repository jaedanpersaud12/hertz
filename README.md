# Hertz

A local TFT stats app and overlay (repository: jaedanpersaud12/hertz).

A local TFT stats app. A crawler collects ranked matches from the Riot API into a SQLite file on your disk,
and the app reads from that file, so every page loads instantly and nothing depends on someone else's server.
The engine (crawler, stats and the app's API) is one Rust program, `tft`; the app is React.

## What is in it

| Page | What it does |
|---|---|
| Comps | Tier list of comps grouped from real boards: traits, core units with stars and items, playstyle tag, avg place, pick rate, win rate. Open a row for final-level breakdown, carry items and flex units. Favourite, pin or hide any comp. |
| Units | Every unit with tier, avg place, win rate, top 4, games. Unit page: ability, base stats, star-level and item-count breakdowns, builds, items, comps it appears in, and patch-by-patch trend. |
| Items | Tier list with place change, type filter and most common holders. Item page: recipe, change since last patch, best holders. |
| Traits | Overall, or one row per breakpoint ("6 Vanguard"). |
| Explorer | Pick up to four units and three traits; see how those boards place, what else is on them, and the items on the first unit. |
| Team builder | Hex board with live trait counting. Saved teams go to My lists. |
| My lists | Favourite comps, saved teams, hidden comps. |
| Match history | Your last games with full lobbies, plus a summoner card (rank, LP, recent placements) in the side panel. |
| Data | How much is stored, and the commands. |

Filters on every stats page: patch, rank (All, Platinum+, Diamond+, Master+), period (whole patch or last 3 days), minimum games.

## The engine and the agent team

The engine is written in Rust (`crates/`, see `crates/README.md`). It replaced a TypeScript engine that matched
it answer for answer; what that engine gave is kept in `test/golden/`, and `npm run golden` checks the Rust
engine still gives exactly that. The work is planned in `docs/plan/` and tracked in Linear, and an agent team
builds it: see `CLAUDE.md`. `npm run gate` is the check every change has to pass.

## Setup

Needs Node 20 or newer and Rust.

1. Install Rust from https://rustup.rs. On Windows, run `rustup-init.exe` and accept when it offers the Visual
   Studio C++ build tools. Open a new terminal afterwards so `cargo` is found.
2. Then:

```
npm install
copy .env.example .env        (macOS/Linux: cp .env.example .env)
```

The first `npm run dev` or `npm start` builds the engine, which takes a few minutes once. Later starts only
rebuild what changed.

Put your Riot API key in `.env`. Get one at https://developer.riotgames.com:

- The **development key** on your dashboard works straight away but expires every 24 hours.
- Register a **personal product** there to get a key that does not expire. The rate limit is the same.

For the summoner card and Match history, also set `RIOT_ID` and `MY_PLATFORM` in `.env`.

## Try it without a key

```
npm install
npm run demo
```

Builds a small database of made-up matches (about 30 seconds, once, plus the first engine build) and opens the app on
http://127.0.0.1:5174. Names and icons are placeholders. Your real database is not touched.

## Daily use

```
npm run build      once, and after updating: builds the app into dist/
npm start          serves the app on http://127.0.0.1:5174 and keeps collecting matches; Ctrl+C stops both
```

`npm start` also rebuilds the stats every 10 minutes when new matches arrived, downloads names, costs,
abilities and icons for the current set at start and every 3 hours, and prunes old raw matches once a day.
Without a Riot API key the app is still served and the crawl says what to do about the key.

To work on the app itself, `npm run dev` runs the same engine behind Vite instead (the app on
http://127.0.0.1:5174, the engine's API on the next port, 5175), with the dev-only stress switch on.

Single jobs, if you want them:

```
npm run crawl            collect matches only; leave it running, Ctrl+C to stop, resumes where it left off
npm run stats            rebuild the numbers the app shows (safe while a crawl is running)
npm run tft -- refresh   download the patch calendar and the set data now
npm run tft -- health    matches per hour per server, matches waiting, the last problem, how old the stats are
npm run tft -- --help    every engine command
```

`npm run tft -- <command>` builds the engine if needed and runs `target/release/tft <command>`
(`tft.exe` on Windows). You can also run that file directly.

### Coming from the first version

Keep your `data` folder and `.env`, replace everything else, then:

```
npm install
npm run recluster     one-off: gives the boards you already stored their comp labels (tft recluster)
npm run stats
```

## How big the dataset gets

Riot's limit for development and personal keys is 100 requests per 2 minutes **per routing group**
(americas, europe, asia, sea). One match costs one request. The default `.env` crawls one server from
each group, so the four limits add up:

| | per group | all four groups |
|---|---|---|
| requests per day | about 68,000 | about 270,000 |
| matches per day | about 60,000 | about 240,000 |
| boards per day (8 per match) | about 480,000 | about 1.9 million |

These are arithmetic from the published limits, not a measured run. Disk use was about 7 KB per match in testing.

`npm run stats` rebuilds 16 views (2 patches x 4 rank filters x 2 periods). On the small test database
it takes a moment; on millions of boards expect minutes. Use `-- --patches 1` to halve it, or
`-- --incremental` to rebuild only the parts with new matches.

## How it works

```
ladders ──> players ──> match ids ──> match_queue ──> matches, participants, units, unit_items, traits
                                                              │
                                              npm run stats   ▼
                                    stat_units, stat_traits, stat_items, stat_comps, ...
                                                              │
                                                    local API ▼  React app
```

- `crates/tft-riot`: Riot client with a per-region rate limiter. It reads the real limits from Riot's
  response headers, so a production key speeds up without code changes. If Riot is unreachable the crawler
  pauses and retries; nothing is skipped. Also the crawler (seeds players from the ladders, then loops per
  routing group) and Community Dragon (patch calendar, set data).
- `crates/tft-core/src/cluster.rs`: the rule that labels a board with a comp (biggest trait plus main carry).
- `crates/tft-stats`: the aggregation SQL, including the step that merges near-identical comps.
- `crates/tft-api`: the local API. Icons are downloaded from Community Dragon once and cached in `data/icons`.
- `crates/tft-cli`: the `tft` command that runs all of it.
- `src/ui`: the app. `tiers.ts` holds the S/A/B/C/D cut-offs and the trait colour table.

### The interface

React 19, Tailwind 4, Motion and the Geist typeface. The look follows Clerk's dashboard: near-black neutral
surfaces, hairline borders, small raised buttons, one violet accent. Gold, green and red are kept for game
results only.

Interactive pieces come from [interior.dev](https://www.interior.dev) (MIT). All 54 components are copied
into `src/ui/interior` as source, recoloured to this palette. In use:

| Where | Component |
|---|---|
| Every stats table | Sortable Table |
| Patch, rank, period, minimum games, sort | Dropdown |
| Cost, item type and trait view switches | Segmented Control |
| Unit page sections | Tabs |
| Comp rows and match history rows | Accordion (its hooks) |
| Search everything, Ctrl or Cmd + K | Command Palette |
| Unit portraits and icon buttons | Tooltip Group |
| Favourite heart | Like Burst (its hook) |
| Every button's press feedback | Press Depth (its hook) |
| Right-click on a comp or a board hex | Context Menu |
| Copy board, copy command | Copy Button |
| Save team, check for new games | Loading Button |
| Delete team, clear board | Hold to Confirm |
| Unit pool in the team builder | Filter Grid |
| Your panel on narrower windows | Drawer |
| Headline numbers on unit and item pages | Value Flash |
| Summoner card while it loads | Skeleton Swap |
| Older games | Load More |
| Setup progress, notices on the Data tab | Task Steps, Collapsible Banner |

The rest are there to pick up as the app grows. `src/ui/parts.tsx` is the thin layer between the app and
the library; `src/ui/styles.css` holds the theme tokens at the top.

### Stress data (dev only)

Under `npm run dev` or `npm run demo`, a plain switch at the bottom of the window swaps the data every page
receives: Demo data, Worst case (long and non-Latin names, missing fields, extreme numbers, a ten-unit board),
Empty, One, and 1,000 rows. The choice lives in the URL (`?data=worst`). The reshaping happens in the engine
(`crates/tft-api/src/stress.rs`, on with `tft run --dev` or `tft serve --dev`), so the real components render
it. It does not exist under `npm start`.

### Motion

Curves are the three `--ease-*` tokens at the top of `src/ui/styles.css`. Things that animate: button press,
a unit landing on the builder board, a comp leaving when hidden, the Explorer picker menu, trait pips changing
colour, plus what the interior.dev components bring (accordion, drawer, dropdown, tooltip, table re-sort).
Deliberately instant: the search palette, the main tabs and every bar or number that is data.

### What the numbers mean

- **Avg place**: mean final placement of boards containing the thing. 4.5 is exactly average.
- **Pick rate** (comps): boards per lobby, from 0 to 8, the same scale MetaTFT uses.
- **Place change** (items and builds): how a unit places holding the item, minus how that unit places
  overall. Negative is better. An item's overall figure is the weighted average of its per-unit changes.
- **Comps**: a board is labelled "biggest trait + main carry". Labels whose usual boards overlap by 75% or
  more are merged. A comp needs 0.2% of boards to be listed. Names are the trait plus the units that
  usually hold three items.
- **Playstyle tag**: "Reroll" when the main carry costs 3 or less and is usually three-star; otherwise
  "Fast 9" or "Fast 8" from the average final level.
- **Patch**: Riot's match data no longer includes a version, so each match is dated to a patch using
  Community Dragon's release calendar. Patch numbers are the client's (16.22), not TFT's set numbering.
- **Rank filter**: a match counts as the rank of the player it was found through. Riot's match data does
  not include ranks, and ranked lobbies are close in rank, so this is an approximation.
- **Tier letters**: our own cut-offs on avg place or place change, not MetaTFT's formula.

## Tested and not tested

`npm test` runs the whole pipeline through the `tft` engine against a fake Riot API and a fake Community Dragon
on your machine and checks: ladder seeding,
staying under the announced rate limit, recovery from 429s and server errors, skipping non-ranked games,
no duplicates on a second run, upgrading an older database, comp grouping, every stat against a straight
recount, and each API route including the summoner card and match history.

Not yet run against the real services (they were not reachable where this was built):

- **The live Riot API.** Endpoint paths and field names follow Riot's documentation.
- **Community Dragon.** Set data downloads (`tft refresh`), ability text clean-up and icon downloads follow its documented
  layout but have only seen sample data. Ability text is the likeliest thing to need a tweak.
- **Trait colours.** The mapping from Riot's trait "style" number to bronze/silver/gold is in
  `src/ui/tiers.ts`. If colours look one step off with live data, change it there.

## Not possible from Riot's match data

These MetaTFT features rely on data its own overlay collects, or are written by hand:

- **Augment stats.** Riot removed augments from match history in November 2024.
- **Early comps, per-stage item stats, average HP, positioning.** Match data has final boards only, with
  no hex positions and no round-by-round state.
- **Pro comps and guide notes.** Hand-written.
- **Difficulty tags.** No data-based definition, so they are left out rather than invented.

## Not built yet

- **In-game overlay and live game reading.** The pinned comp in the side panel is the first piece of it.
- **Team Planner import.** Sending a comp into the League client's planner.
