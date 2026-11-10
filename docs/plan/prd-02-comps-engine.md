# PRD 2: Comps engine and tier list

Weeks 2 to 4 · stories C1 to C14 · depends on PRD 1 (store and stats in Rust).

Boards are grouped into comps by the units they share, each comp gets variants, a levelling guide, item priority and trends, and the tier list becomes filterable and comparable.

## Context

Today a board's comp is "biggest active trait plus the unit holding the most items". Labels that overlap 75% are merged. It is cheap and explainable, and it fails in three ways: flex boards with two carries split into two comps, boards that share a trait but play differently merge into one, and there is no notion of a variant.

A comp is closer to a recipe than a label. Two cooks making the same dish swap an ingredient or two; the engine should see one dish with variations, not two dishes.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Coverage | Share of top 4 boards assigned to a listed comp | 90% or more (baseline measured in week 2) |
| Separation | Core-unit overlap between any two listed comps | Under 75% |
| Stability | Comps that keep their id from one stats rebuild to the next on the same patch | 100% |
| Readable names | Top 30 names reviewed by you | 27 of 30 accepted without rename |
| Speed | Clustering one patch scope of 8 million boards | Under 2 minutes |

## Non-goals

Early-game boards and positioning from capture (PRD 7). Written guides (PRD 9). Predicting what will be strong next patch.

## How clustering works

1. **Signature.** Each board becomes a weighted set of units: weight = cost, plus a bonus for 3 items, plus a small bonus for 3 stars. Low-cost filler units weigh little, carries weigh a lot.
2. **Bucket.** Boards are bucketed by their two heaviest units. This is fast and gets most boards near their final group.
3. **Merge.** Buckets whose typical signatures are closer than a threshold (weighted Jaccard) are merged, most similar first, until none are left under the threshold.
4. **Core.** A comp's core is the units present on at least 70% of its boards. Flex is everything between 20% and 70%.
5. **Variants.** Inside a comp, boards are split by which flex units they run. A variant needs 50 games and a different unit in at least one slot of cost 4 or 5, or a different three-item carry.
6. **Name.** Top trait by unit count, plus the three-item units by frequency. Your renames (C14) override this.
7. **Identity.** A comp id is a hash of its sorted core units. Across patches, a comp inherits the id of last patch's comp with the highest core overlap above 60%.

Today's rule stays in the codebase as the baseline. The week 2 experiment prints both lists side by side on real data; the new method ships only if it wins on coverage and on your read of the top 30.

## Tier score

```
score = 0.55 * z(avg_place, inverted) + 0.25 * z(top4_rate) + 0.20 * z(win_rate)
confidence = games / (games + 400)
final = score * confidence
```

S is the top 8% of listed comps by final score, A the next 17%, B 30%, C 30%, D the rest. The weights and cut-offs live in one config file and are shown in the README, since they are opinion, not fact.

## Comp page sections

| Section | Source | Story |
| --- | --- | --- |
| Header: name, tier, playstyle, avg place, top 4, win, pick rate, trend | Comp stats | C3, C4, C10 |
| Board: core units with stars and items on a hex grid | Most common final board | C1 |
| Variants | Variant split | C2 |
| Levelling: final level distribution by placement band | Participants (level, last round) | C5 |
| Carries: top builds and best single items | Unit items within the comp | C6 |
| Item holders | Low-cost units sharing trait or item class with each carry | C7 |
| Flex: add at 9, swap options | Unit presence by level, place change | C8 |
| Special items: emblems, artifacts, radiants | Unit items filtered by type | C9 |
| Notes | Your notes, when PRD 9 lands |  |

## Data model

```
stat_comps        + core TEXT, flex TEXT, score REAL, tier TEXT, prev_place REAL, d3_place REAL
stat_comp_variants (scope, comp_id, variant_id, units TEXT, games, place_sum, top4, wins)
stat_comp_holders  (scope, comp_id, carry, holder, games, place_sum)
stat_comp_special  (scope, comp_id, item, holder, games, place_sum)
comp_overrides     (comp_id, name, merge_into)          -- yours, never rebuilt
comp_lineage       (patch, comp_id, prev_patch, prev_comp_id, overlap)
```

## Edge cases

| Case | Behaviour |
| --- | --- |
| First 48 hours of a patch, few games | List shows last patch's comps marked "carried over" until 20,000 boards exist |
| A comp falls under the listing threshold | Stays visible if favourited or pinned, marked "low data" |
| Reroll comp and its fast-9 cousin share a core | Kept apart when the three-star carry differs; this is what the star bonus in the signature is for |
| Board with no three-item unit | Assigned by units alone; excluded from carry and build stats |
| Your merge creates a comp over the overlap limit | Allowed. Overrides beat rules |

## Test plan

- **Fixture:** hand-built boards for 6 known comps including one reroll and fast-9 pair; assert they cluster as labelled.
- **Determinism:** two rebuilds of the same data give identical ids and order.
- **Recount:** every comp stat equals a direct SQL count over its member boards.
- **Review:** side-by-side printout of old and new lists for you, top 30 each.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Signature, bucketing, merge; side-by-side report | C1 |
| 2 | Tuning with you on real data; ids and lineage | C1, C13 |
| 3 | Variants; tier score | C2, C3 |
| 4 | Trend columns, filters | C4, C11 |
| 5 | Levelling, carries, holders | C5, C6, C7 |
| 6 | Flex, special items, tags | C8, C9, C10 |
| 7 | Compare view, rename and merge | C12, C14 |

## Open questions

- Should pick rate stay on MetaTFT's 0 to 8 scale (boards per lobby) or switch to a percentage? Currently 0 to 8.
- Is 50 games the right floor for a variant at Master+ where samples are small?
