# PRD 9: Curated content

Weeks 10 to 11 · stories G1 to G9 · depends on PRD 2 (comp ids) · G4 is better with PRD 7, G8 needs PRD 8 (followed players).

Everything that is judgement rather than measurement gets a home: your notes, augment tiers, imported lists, generated summaries. All of it is labelled by where it came from.

## Context

A good share of what makes MetaTFT useful is editorial: pro comps, augment tier lists, short how-to-play notes, difficulty tags. No amount of crawling produces these. The honest equivalent for a personal app is a notes layer you own, imports you choose, and generated text that is clearly marked and built only from our own numbers.

Think of the stats as a map and this PRD as the margin where you write "bridge out, go left".

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Notes are used | Pinned comp panel shows your note when one exists | Always |
| Augments usable in game | Time to find an augment's tier in the overlay | Under 2 seconds by typing |
| Provenance | Content items without a source label | 0 |
| Survives patches | Notes and tiers kept across a patch for entities that still exist | 100% |

## Non-goals

Scraping or mirroring another site's guides or tier lists. A public guide library. Comments or sharing.

## Content types

| Type | Attached to | Source label | Story |
| --- | --- | --- | --- |
| Note | Comp, unit, item, trait, augment | Mine | G1 |
| Guide | Comp | Mine, or Generated then edited | G2, G6 |
| Augment tier list | Set and patch | Mine, or Imported from a named file | G4, G5 |
| Comp list | Patch | Imported | G5 |
| High-elo boards | Comp | Data: Challenger games in the database | G7 |
| Followed players' comps | Player | Data | G8 |
| Patch summary | Patch | Generated from stats | G9 |

## Requirements

### Notes and guides (G1, G2)

- Markdown editor with a live preview, autosave, and unit, item and trait mentions that render as the app's chips.
- The guide template offers five optional headings: early game, levelling, items, positioning, when to pivot.
- Notes follow a comp across patches through its lineage (PRD 2). If a comp disappears, its note moves to an archive list rather than being deleted.
- The overlay's pinned panel shows the first 400 characters with a hotkey to expand.

### Augment browser and tiers (G3, G4)

- The static step stores augments from the Community Dragon set file: id, name, text, tier (silver, gold, prismatic), icon.
- Browser: search by name or text, filter by tier.
- Tier list: drag augments into S to D rows per augment tier. Unranked augments sit in a tray.
- When capture data exists, each augment shows your picks and avg place beside it.
- In game, the lookup panel answers an augment search with your tier and note. Whether it may show during the augment choice itself depends on the policy read.

### Import (G5)

```
{ "kind": "augment-tiers" | "comp-list", "set": 18, "patch": "16.22", "source": "name of the list",
  "tiers": { "S": ["TFT_Augment_..."], "A": [...] } }
```

- Import shows a preview with unknown ids highlighted, the source name and the patch, and asks before saving.
- Imported lists are stored beside yours, never merged into them. A toggle picks which one the overlay uses.

### Generated text (G6, G9)

- **Guide draft.** Input is the comp's own stat block: core, variants, carries and builds, levelling split, holders, trend. Output is a draft in the guide template. Nothing is saved until you accept, and the saved guide carries a "generated, edited by you" label.
- **Patch summary.** Biggest movers by avg place and pick rate for units, items and comps against the previous patch, minimum sample enforced, written as a short list. The rule-based list ships first; prose on top is optional.
- The model is called with your own key, set in Settings. With no key, both features fall back to the rule-based output.
- Prompts state that only supplied numbers may be cited. The draft view shows the stat block next to the text so claims can be checked.

### High-elo and followed boards (G7, G8)

- Per comp: the 20 most recent top 4 boards by Challenger and Grandmaster players in the database, with player, region, placement and date.
- A Pros tab lists followed players with their last 10 games and most played comps this patch.

## Data model

```
notes        (kind, entity_id, body_md, source, updated_at)
aug_tiers    (list_id, set_number, patch, source, name)
aug_tier_rows(list_id, augment, tier, position)
imports      (id, kind, source, patch, imported_at, raw_json)
generated    (kind, entity_id, patch, model, input_json, output_md, accepted)
```

## Edge cases

| Case | Behaviour |
| --- | --- |
| Augment removed or renamed in a patch | Row greyed with "not in current patch"; kept in the list |
| Import for a different set | Refused with the reason |
| Model returns numbers not in the input | Draft view highlights any number absent from the stat block |
| No model key | Buttons read "Build from stats" and produce the rule-based version |
| Very long note | Overlay truncates; the main app does not |

## Test plan

- **Round trip:** export then import a tier list; identical.
- **Lineage:** fixture with a comp renamed across patches; note follows.
- **Number check:** unit test for the highlighter using a draft with a planted false figure.
- **Screenshot:** editor, augment tier board, pinned panel with a long note.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Augments in static data; browser | G3 |
| 2 | Notes and guide template; overlay display | G1, G2 |
| 3 | Augment tier board; import with preview | G4, G5 |
| 4 | High-elo boards; Pros tab | G7, G8 |
| 5 | Patch summary (rules), then generated drafts | G9, G6 |

## Open questions

- Which model and key for G6. Your call; the app only needs an endpoint and a key.
- Whether difficulty should be a tag you set (planned) or dropped entirely.
