# Weekly plan

Each week has a theme, a fixed set of stories, and one checkpoint you can verify yourself on Sunday. Story ids refer to the tables on the Overview tab.

## Rhythm

- **Monday:** pick the week's stories, run any spike first. Spikes are time-boxed to one session and end in a written yes or no.
- **Weekdays:** 1 to 3 stories a day, one commit each.
- **Saturday:** the long session. Biggest story of the week.
- **Sunday:** checkpoint, play-test on real data, count stories done against planned, write three lines in the log at the bottom of this tab.

If a week ends more than 3 stories short, cut from the list on the Overview tab before starting the next week. Do not carry debt forward silently.

## Phase 1: data parity

| Week | Dates | Theme | Stories | Sunday checkpoint |
| --- | --- | --- | --- | --- |
| 1 | 16 to 22 Nov | Rust foundations | Spikes: Riot policy read, set schedule. Workspace set up. `tft-core`, `tft-store`, `tft-riot`. Parity harness. E1 (store and ingest half), E2, E3, E4 | Rust crawler stores real matches into the existing database; ingest parity diff is empty |
| 2 | 23 to 29 Nov | Stats and API in Rust | `tft-stats`, `tft-api`, `tft-cli`. E1 (complete), E5, E6, E11. Clustering experiment for C1 on real data | React app runs against the Rust API with no visible change; TypeScript engine switched off |
| 3 | 30 Nov to 6 Dec | Scale, and comps v2 | E7, E8, E9, E10, E12. C1, C2, C3, C4, C13. U1, U2 | 500,000+ matches stored; new comp list reviewed by hand on the top 30 |
| 4 | 7 to 13 Dec | Depth and Explorer | C5 to C12, C14. U3 to U13. X1 to X8 | Gate 1: every Phase 1 goal measured and written down |

Week 4 is the heaviest week of the plan at 28 stories. Most are queries plus a table, which is where AI sessions go fastest, but it is the first place to apply the cut list (U11 to U13 first).

## Phase 2: app and overlay

| Week | Dates | Theme | Stories | Sunday checkpoint |
| --- | --- | --- | --- | --- |
| 5 | 14 to 20 Dec | Shell and spikes | Spikes: overlay window over a live game, client local API (L1), Team Planner code format. D1, D2, D3. B1, B2, B3 | App installs and crawls from the tray; an empty overlay window sits over a real game without stealing focus |
| 6 | 21 to 27 Dec | First overlay panels | D4, D5, D6, D8, D9. B4, B5, B6, B7 | You play a ranked game with the pinned comp and item panels, and paste a comp into Team Planner |
| 7 | 28 Dec to 3 Jan | Overlay complete, profile | D7, D10, D11, D12. B8, B9, B10. P1, P2, P3, P4 | Your LP graph and per-comp stats are on screen; overlay survives a stats rebuild mid-game |
| 8 | 4 to 10 Jan | Scouting and hardening | D13. P5 to P12 | Gate 2: 10 games with the overlay, zero crashes, zero focus steals, no terminal |

Weeks 6 and 7 fall over Christmas and New Year, so they hold 9 and 11 stories.

## Phase 3: own data and polish

| Week | Dates | Theme | Stories | Sunday checkpoint |
| --- | --- | --- | --- | --- |
| 9 | 11 to 17 Jan | Capture core | L2, L3, L6, L13, L14. G3 | Augment picks recorded for 5 games in a row; stage counter matches the game |
| 10 | 18 to 24 Jan | Boards and timeline | L4, L5, L7, L8. G1, G2, G4, G5 | A finished game opens as a round-by-round timeline; your augment tier list shows in the overlay |
| 11 | 25 to 31 Jan | Personal insights, content | L9, L10, L11, L12. G6, G7, G8, G9. Accuracy check on 20 labelled games | Capture accuracy numbers written down; patch summary generated for the live patch |
| 12 | 1 to 7 Feb | Finish | Q1 to Q10. Bug bash. Clean-machine install test | Gate 3: success measures on the Overview tab checked one by one |

## Standing jobs every week

- Leave the crawler running. Data volume is a function of days, not effort.
- After any patch: check static data loaded, comp ids carried over (C13), and, from Phase 3, capture regions still line up.
- Keep the stress-data switch and screenshot tests green. They are what lets AI sessions move fast without breaking earlier weeks.

## Log

| Week | Planned | Done | Cut or moved | Notes |
| --- | --- | --- | --- | --- |
| 1 | 4 + harness |  |  |  |
| 2 | 4 + crates |  |  |  |
| 3 | 12 |  |  |  |
| 4 | 28 |  |  |  |
| 5 | 6 + 3 spikes |  |  |  |
| 6 | 9 |  |  |  |
| 7 | 11 |  |  |  |
| 8 | 9 |  |  |  |
| 9 | 6 |  |  |  |
| 10 | 8 |  |  |  |
| 11 | 8 |  |  |  |
| 12 | 10 |  |  |  |
