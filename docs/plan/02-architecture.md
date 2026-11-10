# Architecture

One Rust process owns every input and the single database file; the React app and the overlay only ever read through it.

&#91;embedded content: architecture · 3 sources, 6 crates, 3 clients\]

Read it top to bottom: three outside sources feed three crates, all of which write into one store. Stats are computed from the store and written back. The API serves the main window and the overlay; the command line drives the same engine without the app.

## Why this shape

- **One writer.** Only the engine writes to SQLite. Clients cannot corrupt or lock it, and the overlay reads from a snapshot (WAL) even mid-rebuild.
- **One binary.** The Tauri app links the engine crates directly. The command line is the same crates behind a different front door.
- **Clients are thin.** The React code that exists today keeps working because the routes keep their shapes.
- **Data files, not code, for what changes per set.** HUD layouts, Team Planner tables and tier cut-offs are files the engine loads.

## Repository layout

```
tft-local/
  crates/
    tft-core/      types, comp signature, patch calendar, config
    tft-riot/      client, limiter
    tft-store/     schema, migrations, ingest, queries
    tft-stats/     aggregation, clustering, explorer index
    tft-api/       axum routes, Tauri command wrappers
    tft-capture/   client API, logs, screen, recognition
    tft-cli/       binary: tft
  app/
    src-tauri/     shell, tray, window manager, hotkeys, updater
    src/           React UI (today's src/ui)
    overlay/       panel entry points sharing src components
  data-files/      layouts/, planner/, tiers.toml
  fixtures/        saved matches, golden API responses, recorded games
  legacy-ts/       today's engine, kept until parity passes, then deleted
```

## Data model by group

| Group | Tables | Written by | Rebuilt? |
| --- | --- | --- | --- |
| Crawl state | players, match\_queue | tft-riot | No |
| Raw matches | matches, participants, units, unit\_items, traits | tft-store ingest | No, pruned after 3 patches |
| Static | static\_data, patch calendar (kv) | tft-core | Refreshed per patch |
| Stats | stat\_\* tables, stat\_watermark, explorer index file | tft-stats | Yes, from raw |
| Yours | comp\_marks, comp\_overrides, saved\_teams, notes, aug\_tiers, followed, settings | API on your actions | Never. This is what backup exports |
| Profile | me\_matches, lp\_snapshots, scout\_cache | tft-riot priority jobs | Partly (cache) |
| Capture | cap\_games, cap\_rounds, cap\_boards, cap\_augments, cap\_unsure | tft-capture | Never |

The rule that matters: anything in the "Yours" and "Capture" groups is irreplaceable, everything else can be refetched or recomputed.

## API surface

| Area | Routes | Notes |
| --- | --- | --- |
| Meta | `/api/meta`, `/api/static`, `/api/icon/:kind/:id` | Unchanged |
| Stats | `/api/comps`, `/api/comps/:id`, `/api/units`, `/api/units/:id`, `/api/items`, `/api/items/:id`, `/api/traits` | Same shapes, new fields added |
| Explorer | `POST /api/explore` | New query model (PRD 4) |
| Yours | `/api/lists`, `/api/teams`, `/api/notes`, `/api/augments/tiers` |  |
| Profile | `/api/me`, `/api/players/:id`, `/api/matches/:id`, `/api/scout` |  |
| Engine | `/api/engine/status`, `/api/engine/settings`, `/api/engine/log` | Drives the Data and Settings pages |
| Capture | `/api/capture/status`, `/api/capture/games/:id` |  |
| Events | One server-sent stream: game state, capture readings, crawl progress | The overlay subscribes instead of polling |

Inside the Tauri app the same handlers are also exposed as commands, so the overlay skips HTTP entirely. The HTTP server binds to loopback only.

## Process and threads

| Task | Runs | Priority |
| --- | --- | --- |
| Crawl loop per region (4) | Always | Low |
| Priority jobs (your refresh, scouting, lookups) | On demand | High, pre-empt the home region's crawl |
| Stats tick | Every 10 minutes when new matches exist | Low, paused in game if the setting says so |
| Static and calendar refresh | Every 3 hours | Low |
| Game state watcher | While the client is open | High |
| Capture | While in game | High, trigger-based |
| API | Always | High |

## Decisions recorded

| Decision | Choice | Why | Revisit if |
| --- | --- | --- | --- |
| Engine language | Rust | Single binary with Tauri, low idle cost, native capture | Port blows past week 2 |
| Database | SQLite, WAL, one file | Already proven, zero setup, fast enough with the index | Stats rebuild exceeds 10 minutes |
| Desktop shell | Tauri 2 | Rust backend, small installer | Overlay spike fails on the chosen platform |
| UI | Existing React, Tailwind, interior.dev components | Already built and styled | Never, short of a rewrite |
| Explorer | In-memory bitmap index, memory-mapped | Sub-second on 8 million boards | Memory pressure while in game |
| Capture | Client API first, screen second, never memory | Policy and anti-cheat | Riot publishes a richer local API |
| Hosting | None. Everything local | Speed, privacy, no bill | You decide to release it |
