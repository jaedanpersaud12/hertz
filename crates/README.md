# The Rust engine

The whole engine behind the React app in `src/ui`: crawler, stats and the app's API, in one program, `tft`.
It replaced the TypeScript engine, which was deleted once this matched it exactly; what that engine gave is kept
in `test/golden/` (see Checks). Plan: `docs/plan/prd-01-rust-engine.md`.

| Crate | Job | State |
| --- | --- | --- |
| `tft-core` | Settings, Riot response shapes, comp labels, patch dating, dictionary string order | Done |
| `tft-store` | SQLite schema, migrations, storing matches, health, pruning | Done |
| `tft-riot` | Riot client, per-region rate limiter, Community Dragon, the crawler and its supervisor | Done |
| `tft-stats` | Aggregation, comp merging, incremental builds (`tft stats`) | Done; Explorer index comes in E5 |
| `tft-api` | The `/api` routes the React app calls, the built app, and the dev-only `?data=` stress switch | Done |
| `tft-cli` | The `tft` command | Done |

## Use it

Needs Rust (https://rustup.rs). On Windows the installer also offers the Visual Studio C++ build tools; accept.

Most of the time you do not call it yourself: `npm start` runs `tft run`, `npm run dev` runs `tft run --dev`
next to Vite, and `npm run tft -- <command>` runs any command below (each builds the engine first if needed).

```
cargo build --release -p tft-cli          # builds target/release/tft (tft.exe on Windows)
target/release/tft run                    # serve the app and keep collecting matches; Ctrl+C stops both
target/release/tft run --dev              # the same for `npm run dev`: API alone on PORT+1, ?data= stress switch on
target/release/tft crawl                  # collect matches until Ctrl+C; stats rebuilt every 10 minutes when new ones arrive
target/release/tft crawl --once           # stop when every known player is up to date, then build the stats once
target/release/tft crawl --no-supervisor  # collect matches only: no stats builds, data refresh or pruning
target/release/tft stats                  # rebuild the stat tables for the 2 most recent patches
target/release/tft stats --incremental    # rebuild only the parts with new matches, and list them
target/release/tft serve                  # serve the app and its API on 127.0.0.1, without crawling
target/release/tft serve --dev            # the API alone on PORT+1, behind Vite, with the ?data= stress switch
target/release/tft refresh                # download the patch calendar and the set data (names, icons) now
target/release/tft recluster              # label every stored board with its comp again, then run tft stats
target/release/tft health                 # matches per hour per server, matches waiting, the last problem, stats age
target/release/tft patches                # download the patch calendar, date matches stored without one
target/release/tft ingest saved.json      # store saved match files
target/release/tft --help                 # every command; `tft <command> --help` for its options
```

Settings come from `.env` in the current folder (see `.env.example`); the database is `data/tft.db` unless
`DB_PATH` says otherwise. `tft run` works without a Riot API key: the app is served and the crawl says what to
do about the key. `CDRAGON_BASE` points the Community Dragon downloads at another address (the tests use a fake).

## Checks

```
npm run test:rust     cargo test and clippy
npm run golden        the fixtures through tft, compared with test/golden/ (what the TypeScript engine gave)
npm run resume        a crawl killed at any point and started again stores what one clean crawl stores
npm run cli           the tft commands against the fake Riot API
npm test              the whole pipeline end to end, against the fake Riot API and a fake Community Dragon
```

`npm run golden -- --update` rewrites `test/golden/` from what the engine gives now. That changes what the
engine is checked against, so it is a deliberate story of its own (hard rule 2 in `CLAUDE.md`: changing what a
number means is a separate, explicit story), never a way to make a failing gate pass. Its review lists which
entries changed and why.
