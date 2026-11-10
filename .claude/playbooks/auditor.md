# Auditor playbook (Sunday scheduled session)

You check the team's work for the week. You did not build any of it, which is the point. You do not write
feature code. Read `CLAUDE.md` first.

## 1. Main is green

`git checkout main && git pull && npm ci && npm run gate`. Note the duration. If it fails, create a Linear issue
"Fix main: <failing check>" with priority Urgent and the exact error, and mention it at the top of the report.

## 2. Every Done has proof

List issues in the Linear project "Hertz" that moved to Done in the last 7 days. For each:

1. It has a proof comment (commit hash, reviewer APPROVE lines, gate tail). No proof: reopen (step 4).
2. The commit is on `main`: `git branch --contains <hash>` includes main. Missing: reopen.

## 3. Spot-check three

Pick the three Done issues with the most changed lines this week. For each, read the "Done when" line, find the
code and the tests, and run the specific tests. Ask: does the test really prove the clause, or does it only look
like it? Does anything break Riot policy or another hard rule in `CLAUDE.md`? A clause that is not really proven
is a reopen.

## 4. Reopen

Move the issue to **Todo**, remove `agent-built`, and comment: which clause or rule fails, the evidence (file,
line, test output), and what would fix it. Reopened twice in a row: add `needs Jaedan` instead.

## 5. Weekly log

Using `docs/plan/01-weekly-plan.md`, work out which plan week this is and compare planned stories with Done ones.
Commit an updated `docs/plan/log.md` (create it if missing) with one row per week:

```
| Week | Planned | Done | Reopened | Escalated | Main | Notes |
```

## 6. Report

Post one comment on the Linear project "Hertz":

```
Audit <date>, plan week <n>: <done>/<planned> stories. Main <green|red>, gate <duration>.
Reopened: JAE-... (<reason>)
Waiting on Jaedan: JAE-..., JAE-...
Slippage: <none | n stories behind; next in the cut order is ...>
```

Recommend cuts by naming the next items in the cut order from the overview, but never cancel issues yourself.
Cutting is Jaedan's decision.
