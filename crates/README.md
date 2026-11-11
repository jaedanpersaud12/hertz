# The Rust engine

Replaces the TypeScript engine (`src/crawler`, `src/stats`, `src/server`) piece by piece. The React app in
`src/ui` stays. Plan: `docs/plan/prd-01-rust-engine.md`.

| Crate | Job | State |
| --- | --- | --- |
| `tft-core` | Settings, Riot response shapes, comp labels, patch dating, dictionary string order | Done, matches TS |
| `tft-store` | SQLite schema (identical to `src/db.ts`), migrations, storing matches | Done, matches TS |
| `tft-riot` | Riot client, per-region rate limiter, Community Dragon, the crawler | Done, matches TS |
| `tft-stats` | Aggregation, comp merging (`tft stats`); Explorer index and incremental builds come in E5 | Full rebuild done, matches TS |
| `tft-api` | The `/api` routes the React app calls | Week 2 |
| `tft-cli` | The `tft` command | `crawl`, `ingest`, `patches`, `init` |

## Use it

Needs Rust (https://rustup.rs). On Windows the installer also offers the Visual Studio C++ build tools; accept.

```
cargo build --release -p tft-cli          # builds target/release/tft (tft.exe on Windows)
target/release/tft run                    # serve the app and keep collecting matches; Ctrl+C stops both
target/release/tft crawl                  # collect matches until Ctrl+C; stats rebuilt every 10 minutes when new ones arrive
target/release/tft crawl --once           # stop when every known player is up to date, then build the stats once
target/release/tft crawl --no-supervisor  # collect matches only: no stats builds, data refresh or pruning
target/release/tft stats                  # rebuild the stat tables for the 2 most recent patches
target/release/tft stats --incremental    # rebuild only the parts with new matches, and list them
target/release/tft serve                  # serve the app and its read-only API on 127.0.0.1
target/release/tft health                 # matches per hour per server, matches waiting, the last problem, stats age
target/release/tft patches                # download the patch calendar, date matches stored without one
target/release/tft ingest saved.json      # store saved match files
target/release/tft --help                 # every command; `tft <command> --help` for its options
```

Everything reads the same `.env` and database as the TypeScript app, so you can switch engines at any time.
`tft run` works without a Riot API key: the app is served and the crawl says what to do about the key.

## Checks

```
npm run test:rust     cargo test and clippy
npm run parity        both engines on the same input, every row compared
npm run cli           the tft commands against the fake Riot API
```
