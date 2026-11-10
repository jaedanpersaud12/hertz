# The Rust engine

Replaces the TypeScript engine (`src/crawler`, `src/stats`, `src/server`) piece by piece. The React app in
`src/ui` stays. Plan: `docs/plan/prd-01-rust-engine.md`.

| Crate | Job | State |
| --- | --- | --- |
| `tft-core` | Settings, Riot response shapes, comp labels, patch dating, dictionary string order | Done, matches TS |
| `tft-store` | SQLite schema (identical to `src/db.ts`), migrations, storing matches | Done, matches TS |
| `tft-riot` | Riot client, per-region rate limiter, Community Dragon, the crawler | Done, matches TS |
| `tft-stats` | Aggregation, comp merging, Explorer index | Week 2 |
| `tft-api` | The `/api` routes the React app calls | Week 2 |
| `tft-cli` | The `tft` command | `crawl`, `ingest`, `patches`, `init` |

## Use it

Needs Rust (https://rustup.rs). On Windows the installer also offers the Visual Studio C++ build tools; accept.

```
cargo build --release -p tft-cli          # builds target/release/tft (tft.exe on Windows)
target/release/tft crawl                  # same as npm run crawl; reads the same .env and database
target/release/tft crawl --once           # stop when every known player is up to date
target/release/tft patches                # download the patch calendar, date matches stored without one
target/release/tft ingest saved.json      # store saved match files
```

The Rust crawler writes the same database the TypeScript app reads, so you can switch crawlers at any time.
Stats are still built by `npm run stats` until `tft-stats` lands.

## Checks

```
npm run test:rust     cargo test and clippy
npm run parity        both engines on the same input, every row compared
```
