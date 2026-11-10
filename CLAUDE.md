# Hertz (repo: jaedanpersaud12/hertz)

Hertz is the app's name. A private, locally running Teamfight Tactics stats app and in-game overlay, replacing the MetaTFT app for one
player (Jaedan, plays on Windows). Built by an agent team (see "How work gets done" below) with Jaedan as owner.

## Where things are

| Path | What |
| --- | --- |
| `docs/plan/00-overview.md` | The plan: summary, ground rules, **policy check**, roadmap, all user stories with acceptance criteria, risks, cut order |
| `docs/plan/01-weekly-plan.md` | Which stories belong to which week |
| `docs/plan/02-architecture.md` | Crates, data model groups, API surface, recorded decisions |
| `docs/plan/prd-*.md` | One PRD per area: requirements, data model, edge cases, test plan, build order |
| `crates/` | The Rust engine (replacing the TypeScript engine piece by piece) |
| `src/crawler`, `src/stats`, `src/server` | The TypeScript engine. It is the specification until the Rust port passes parity, then it is deleted |
| `src/ui` | The React app. Stays. Design rules in "UI rules" below |
| `test/world.ts` | The deterministic fake TFT world and fake Riot API every test uses |
| `scripts/parity.ts` | Runs both engines on the same input and diffs the results |

The live plan doc (editable, commented) is https://claude.ai/artifact/CWtsQ5zLe8sgPkoHRBuQcQ.
`docs/plan/` is a snapshot of it; if they disagree, ask Jaedan which wins.
Tracking: Linear project "Hertz" (team Jaedan), one milestone per PRD, story id at the start of each issue title.

## The gate

```
npm run gate        # cargo test + clippy -D warnings, parity, TypeScript e2e, app build
```

Nothing reaches `main` unless the gate passes. Run it before you say anything is done. Paste the tail of its output
as proof. No Riot API key exists in the cloud: every test runs against the fake Riot API in `test/world.ts`.

## Hard rules

1. **Riot policy.** In-game panels show static, pre-game information only. No scouting of opponents during play,
   including the loading screen. No recommendations that react to what is happening in the current game. No win
   rates for Legends or Legend-based augments. No reading game memory, ever. Details in the policy check section
   of `docs/plan/00-overview.md`. If a story seems to need any of these, stop and escalate.
2. **Parity first.** Until a TypeScript engine piece is retired, the Rust replacement must match it exactly,
   proven by `npm run parity`. Changing what a number means is a separate, explicit story.
3. **Tests are the contract.** Never weaken, skip or delete a test to make something pass. If a test is wrong,
   say so in the review and let the reviewer agree before changing it.
4. **Stay in scope.** Touch only what the story needs. No drive-by refactors, renames or formatting sweeps.
5. **No secrets in the repo.** No keys, tokens or personal data in code, fixtures or commits.
6. **Agent rules are Jaedan's.** Do not edit `CLAUDE.md` or anything in `.claude/` unless Jaedan asks.
7. **Brand comes from Jaedan.** The Hertz brand (logo, colours, type, art) is being designed separately. Do not
   invent brand assets; use what is in `brand/` once it exists, and keep the current tokens until then.
8. **Data files, not code, for per-set things.** HUD layouts, Team Planner tables, tier cut-offs live in files.

## UI rules

- Look: Clerk-style near-black neutrals, one violet accent (`#6c47ff`), gold/green/red for results only.
  Tokens at the top of `src/ui/styles.css`. Components come from `src/ui/interior` through `src/ui/parts.tsx`.
- Motion: animate only transform and opacity, with the `--ease-*` tokens. No animation on keyboard-initiated or
  very frequent actions (command palette, tabs, data bars).
- Every new screen has empty, loading and error states that say what to do next, and must hold up under the
  dev stress switch (`?data=worst`, `empty`, `one`, `many`).
- Writing in the UI: plain words, sentence case, say what happens.

## How work gets done (the agent team)

| Role | Runs | Job | May not |
| --- | --- | --- | --- |
| Lead | Daily scheduled session | Picks the next issue, writes the brief, runs builder, tester and reviewer, merges, updates Linear | Write feature code itself |
| Builder | Sub-agent, one per issue | Implements the brief on a branch, tests first | Merge, review its own work, touch files outside the brief |
| Tester | Sub-agent | Runs the gate on the branch, reports pass or fail with output | Change code |
| Reviewer | Fresh sub-agent | Checks the diff against the story's acceptance criteria and these rules | See the builder's reasoning, change code |
| Auditor | Weekly scheduled session (Sunday) | Re-checks everything marked Done, reopens anything without proof, writes the weekly log | Build features |
| Jaedan | Whenever | Issues labelled `needs Jaedan`, cut decisions, anything escalated | |

Two failed attempts on one issue means it goes to Jaedan (`needs Jaedan` label plus a comment), not a third try.
Procedures: `.claude/playbooks/lead.md` and `auditor.md` for the scheduled sessions, `.claude/agents/` for the sub-agents.

## Commands

```
npm run gate                  everything below, in order
npm run test:rust             cargo test, clippy
npm run parity                Rust engine against the TypeScript engine
npm test                      TypeScript end-to-end test against the fake Riot API
npm run build                 the React app
cargo run -p tft-cli -- --help   the Rust engine's command line
```
