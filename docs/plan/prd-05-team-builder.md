# PRD 5: Team Builder and Team Planner export

Weeks 5 to 6 · stories B1 to B10 · depends on PRD 4 (index) and PRD 2 (comp ids) · feeds the overlay's pinned panel in PRD 6.

The builder becomes where a game is prepared: build a board, see how real boards like it place, plan it level by level, and send it to the in-game Team Planner.

## Context

The builder has a hex board, a unit pool, live trait counting and saved teams. It is a drawing tool. It does not know whether the board is good, cannot hold items or emblems, and the result has to be rebuilt by hand in the client.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Feedback speed | Time from placing a unit to updated stats | Under 100 ms |
| Round trip | Build a comp, copy code, paste in client | Works for every unit in the current set |
| Planning | A saved team holds boards for levels 6 to 9, items and positions | Yes |

## Non-goals

Fight simulation. Positioning advice. Sharing teams through any server.

## Requirements

### Board and pool

1. Drag from pool to hex, hex to hex, hex to pool. Click-to-place for keyboard and trackpad.
2. Star level toggle per unit (1, 2, 3). Affects stats queries, not trait counts.
3. Items: drag from an item tray onto a unit, maximum 3. The components needed for all items on the board are totalled under it (B3).
4. Emblems add their trait to the holder. A "+1 trait" control per trait covers set mechanics that grant traits by other means (B4).
5. Level selector 1 to 10 caps the unit count and drives suggestions.

### Live stats panel (B1)

- Runs an Explorer query with the board's units as conditions. Shows games, avg place, top 4, and the nearest listed comp with overlap percentage.
- If fewer than 30 boards match all units, it drops the lowest-cost unit and retries, and says which units it ignored.

### Suggestions (B2)

- Top 5 units to add: Explorer "units" grouping on the current board, ranked by place change, filtered to units that fit the level's shop odds.
- Each suggestion shows the trait breakpoints it would complete.

### Level plan (B9)

- A team holds up to four boards: levels 6, 7, 8, 9. Copying a board forward to the next level is one click.
- The overlay's pinned panel steps through these.

### Team Planner codes (B5, B6)

- Copy produces a code the client's Team Planner accepts from the clipboard. Import parses a code into the board.
- The format changes with each set, so the unit table is data, not code: one file per set, generated from Community Dragon where possible.
- Team Planner has no positions, items or stars. Import fills units only and keeps whatever positions the builder assigns.

### Sharing (B10)

- Image export renders the board with items and traits at 2x.
- Link export encodes the team in the URL fragment. Nothing leaves the machine.

## Data model

```
saved_teams  + items TEXT, stars TEXT, emblems TEXT, levels TEXT (JSON: {"6":[...],"7":[...]}), source_comp TEXT
planner_sets (set_number, table_json, verified_at)
```

## Spike: Team Planner format (first session)

1. Build three known teams in the live client and copy their codes.
2. Decode: find how units are indexed and where the set marker sits.
3. Check whether the index order can be derived from Community Dragon data.
4. Write the result into this PRD. If the order cannot be derived, the table is maintained by hand once per set (about 60 entries).

## Edge cases

| Case | Behaviour |
| --- | --- |
| Code from a different set | Clear message naming the set; nothing imported |
| Unit in the builder that Team Planner cannot hold | Skipped on export with a note listing skipped units |
| More units than the level allows | Allowed, with the count shown in red; export still works |
| Opening a comp whose units were removed this patch | Missing units shown as empty hexes with their old name |
| Board with zero units | Stats panel shows the overall average and the prompt to add a unit |

## Test plan

- **Round trip:** encode then decode every unit in the set; compare.
- **Client check:** paste 5 exported codes into the live client by hand each set.
- **Screenshot:** builder at 3 widths with the worst-case stress data (10-unit board, long names).
- **Trait counting:** fixture of emblem and +1 cases against expected breakpoints.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Planner format spike |  |
| 2 | Live stats panel on the index | B1 |
| 3 | Suggestions | B2 |
| 4 | Items, components, emblems | B3, B4 |
| 5 | Planner export and import | B5, B6 |
| 6 | Open from comp, positions, level plan | B7, B8, B9 |
| 7 | Image and link sharing | B10 |

## Open questions

- Should suggestions favour the pinned comp's units over raw place change? Probably a toggle.
- Do saved teams need folders, or is a flat list with search enough for now?
