# PRD 3: Units, items, traits and tables

Weeks 3 to 4 · stories U1 to U13 · depends on PRD 1; U2 depends on PRD 2 (comp ids).

Unit, item and trait pages gain the cuts players decide with, and every table in the app gets the same set of tools.

## Context

The pages show headline numbers: avg place, win rate, top 4, games, builds, items. The decisions players actually make are conditional: "I have these two items, what is the third", "is it worth three-starring this", "who holds this emblem". Each of those is one more dimension on data we already store.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Decision in 3 clicks | Clicks from Units list to "best third item given two" | 3 or fewer |
| Table speed | Frame time scrolling 1,000 rows | Under 16 ms |
| Honest numbers | Rows under minimum games shown without dimming | 0 |
| Page load | Any unit, item or trait page | Under 100 ms |

## Non-goals

Per-stage item stats (needs capture). Augment interactions. Anything about positioning.

## Requirements by page

### Unit page

- **Star by items grid (U1).** A 3 by 4 grid: star level 1 to 3 against 0 to 3 items. Each cell shows avg place and games. Cells under the minimum are dimmed.
- **Builds (U2).** Three-item builds ranked by place change, with a comp filter. Under 50 games in the chosen comp it falls back to all boards and says so.
- **Item completer (U3).** Pick up to two items the unit already holds; the list shows the best items to add, by place change, with games.
- **Teammates (U4).** The 10 units this unit places best alongside, minimum 200 shared boards, as place change against the unit's own average.
- **Trend.** Patch-by-patch avg place and pick rate, already present, extended to the 3-day window.

### Item pages

- **Components (U5).** For each of the base components: the completed items it builds, each weighted by its place change on the units of currently listed S and A comps. This gives a carousel priority that follows the meta.
- **By type (U6).** Tabs for craftable, radiant, artifact, support, emblem. Each item lists its 5 best holders.
- **Emblems (U7).** Best holders, plus the breakpoint the emblem most often completes on winning boards.

### Trait page

- **Breakpoints (U8).** One row per active breakpoint with avg place, pick rate and the most common unit set at that count.

### Every table

- **Columns, sticky header, saved sort (U9).** Stored per table id in local settings.
- **Virtual rows (U10).** Only visible rows are in the DOM. Sorting stays animated under 100 rows and becomes instant above, following the app's motion rules.
- **CSV export (U11).** Visible columns and current filters, with a header line stating patch, rank and period.
- **Thin-sample dimming (U12).** One shared rule, driven by the minimum-games setting in the top bar.
- **Patch notes (U13).** Lines for this entity from the current patch, when a source exists.

## Data model

```
stat_unit_grid   (scope, unit, star, n_items, games, place_sum, top4, wins)
stat_unit_pairs  (scope, unit, item_a, item_b, item_c, games, place_sum)   -- item_c NULL = any
stat_teammates   (scope, unit, other, games, place_sum)
stat_components  (scope, component, item, weight, games)
stat_trait_sets  (scope, trait, tier, units TEXT, games, place_sum)
patch_notes      (patch, kind, id, text)
```

`stat_unit_pairs` and `stat_teammates` grow with the square of items and units. Rows under the minimum games for the scope are not stored. Expected size is under 300,000 rows per scope.

## Edge cases

| Case | Behaviour |
| --- | --- |
| Unit has duplicate items (two of the same) | Pairs treat duplicates as a real pair, shown as "x2" |
| Units that cannot hold items or are summoned | Excluded from build and pair stats by a flag in static data |
| Item removed mid-patch | Stays in old patch scopes, hidden from the current one |
| Trait with a single breakpoint | Breakpoint table collapses to one row, no tabs |
| No patch notes source found | U13 hidden entirely; no empty panel |

## Test plan

- **Recount:** each new stat table equals a direct count over raw rows on the fixture.
- **Screenshot:** unit, item and trait pages under the worst-case, empty and one-row stress modes.
- **Performance:** scripted scroll of the 1,000-row stress table, assert no frame over 16 ms after first paint.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Grid and builds-by-comp queries and UI | U1, U2 |
| 2 | Pair table, item completer | U3 |
| 3 | Teammates, trait sets | U4, U8 |
| 4 | Components, type tabs, emblems | U5, U6, U7 |
| 5 | Table toolkit: columns, sticky, virtual rows, dimming | U9, U10, U12 |
| 6 | Export; patch notes source spike | U11, U13 |

## Open questions

- Is there a stable machine-readable source for TFT patch notes? If not, U13 is cut.
- Should place change be shown against the unit's average (today) or against the same unit at the same star level? The second is fairer and needs the grid from U1.
