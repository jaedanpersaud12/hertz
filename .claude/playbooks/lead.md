# Lead playbook (daily scheduled session)

You run the TFT Local build for today. You plan, brief, coordinate and record. You do not write feature code.
Read `CLAUDE.md` first; its hard rules override everything here.

## Limits

- At most **6 issues** reach Done per run, and stop starting new issues after about 4 hours.
- At most **2 attempts** per issue. After the second failure, escalate (step 6) and move on.
- Never start an issue labelled `needs Jaedan`, or one whose dependencies (named in its PRD) are not Done.

## 0. Make sure main is healthy

1. `git checkout main && git pull`.
2. `npm ci && npm run gate`.
3. If the gate fails on `main`, today's only job is fixing it: create a Linear issue "Fix main: <failing check>",
   run it through steps 2 to 5 as the first issue, and do nothing else until `main` is green.

## 1. Pick the next issue

In the Linear project "TFT Local" (team Jaedan), list issues that are not Done, Canceled or labelled
`needs Jaedan`. Order by: In Progress first (finish what was started), then due date, then priority, then the
week order in `docs/plan/01-weekly-plan.md`. Skip any whose dependencies are not Done. Take the first one.

## 2. Brief

1. Move the issue to **In Progress**.
2. Branch: `git checkout -b jae-NNN-<short-slug> main && git push -u origin HEAD`.
3. Write the brief:
   - Issue id, title, the full "Done when" line.
   - The matching section of the PRD in `docs/plan/` (copy the requirement text, data model and edge cases that
     apply; do not just link it).
   - **Allowed files**: the files and folders the change should need. Be specific; this is the scope check.
   - The test you expect to prove it (unit, parity, e2e or screenshot).
   - Anything the last attempt got wrong (on attempt 2).

## 3. Build

Run the `builder` sub-agent with the brief. Wait for its report.

## 4. Check

1. Run the `tester` sub-agent on the branch. FAIL goes back to the builder (step 3) with the tester's report as
   attempt 2.
2. On PASS, run the `reviewer` sub-agent with **only** the issue text, the PRD section, the allowed-files list, the
   branch name and the tester's report. Never pass on the builder's report or your own opinion of the change.
3. `CHANGES` goes back to the builder as attempt 2 with the reviewer's list. `APPROVE` continues.

## 5. Land and record

1. Merge: `git checkout main && git pull && git merge --squash <branch> && git commit` with the message
   `JAE-NNN <story id>: <summary>` and the attribution lines the session asks for, then `git push`.
   If `gh` is authenticated, open and merge a pull request instead, so the review lives on GitHub.
2. Run `npm run gate` on `main` once more. If it fails, revert the merge commit, push, and treat the issue as a
   failed attempt.
3. In Linear: add a comment with **proof** (commit hash, the reviewer's APPROVE lines, the last 15 lines of the
   gate on `main`), add the label `agent-built`, move the issue to **Done**.
4. Delete the branch.

## 6. Escalate

When an issue fails twice, or the builder reports it impossible as written:

1. Leave the branch pushed for inspection; do not merge it.
2. Add the `needs Jaedan` label and a comment: what was tried, what failed (with the exact error or the reviewer's
   items), and the one decision or input Jaedan needs to give.
3. Move the issue back to **Todo**.

## 7. Report the day

Post one comment on the Linear project "TFT Local":

```
Lead run <date>: <n> done, <n> escalated, main <green|red>.
Done: JAE-..., JAE-...
Escalated: JAE-... (<reason in a few words>)
Next up: JAE-...
```

Then stop. Do not start work you cannot finish within the limits.
