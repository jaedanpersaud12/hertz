# PRD 10: Identity, onboarding, settings and quality

Week 12, with pieces in every phase · stories Q1 to Q10 · depends on everything; Q10 starts in week 1.

The app looks like one product from installer to overlay, sets itself up, explains its own failures, and stays fast as features pile on.

## Context

Twelve weeks of AI-built features will drift: slightly different empty states, a table that skipped the shared component, a setting that only exists in a file. This PRD is the weekly tidy-up made explicit, plus the things only a finished product has: onboarding, settings, backup, a complete identity.

Your stated bar is "the cleanest". The existing design system (Clerk-style neutrals, violet accent, cut-stone art, Emil Kowalski motion rules) is the standard everything is held to.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| First run | Install to first real comp list | Under 10 minutes, no terminal, no file editing |
| Speed | Slowest API route on 1 million matches | Under 100 ms |
| Consistency | Screens using ad-hoc buttons, tables, empty states | 0 |
| Real-data polish | Raw ids, broken icons or markup visible on the current set | 0 |
| Safety net | Key screens under screenshot test | 12 |
| Recoverable | Favourites, notes, teams, tiers, captures, settings restored from one backup file | All |

## Non-goals

Mobile layout. Localisation. Accessibility certification (keyboard and contrast are covered; a full audit is not).

## Requirements

### First run (Q1)

1. Welcome with the three things the app needs: API key (with a link and a one-line explanation of dev versus personal keys), Riot ID and region, which regions to crawl.
2. The key is tested live. Errors use the E4 messages.
3. Static data downloads with a progress bar.
4. Crawl starts. A progress view shows matches arriving with an estimate to "enough to show comps" (5,000 matches, roughly 40 minutes).
5. Meanwhile the app opens on demo data with a banner, and swaps to real data when ready.

### Settings (Q2)

| Group | Settings |
| --- | --- |
| Account | API key (keychain), Riot ID, home region |
| Data | Regions, tiers, patches kept, storage cap, pause crawling while in game |
| Stats | Default rank and period, minimum games, tier cut-offs |
| Overlay | Per-panel on or off, hotkeys, opacity, private-only panels |
| Capture | On or off, calibration, keep unsure crops |
| Content | Model endpoint and key, active augment list |
| Appearance | Theme, density, reduced motion |
| Backup | Export, import, open data folder |

Every setting applies without a restart. `.env` is read once for migration and then ignored.

### Identity (Q3)

- Complete set in the cut-stone style: S to D tier badges, placement medals 1 to 8, trait gems by tier, cost frames for unit portraits, empty-state illustrations for each page, app icon, tray icons for crawling, paused and error, installer art.
- Hand-drawn SVG remains the baseline so the set is complete without outside tools. Generated art from Quiver or Higgsfield replaces pieces one by one when supplied.
- The identity sheet route stays as the single place to review all of it.

### Real-data polish (Q4)

Checklist, run against the live set after each patch:

- [ ] Every unit, item, trait and augment has a name and an icon
- [ ] Ability and item text has no markup or placeholder variables
- [ ] Trait colours match the game's bronze, silver, gold, prismatic
- [ ] Comp names read naturally on the top 30
- [ ] Long names truncate cleanly at every width
- [ ] Non-playable or summoned units are excluded where they should be

### Performance (Q5)

- Budgets: 100 ms per route, 100 ms per overlay panel, 500 ms per Explorer query, 16 ms per frame while scrolling, 80 MB engine memory while crawling.
- Route timings are recorded in a ring buffer and shown on the Data page; a dev-mode warning fires when a budget is broken.

### Keyboard (Q6)

- Command palette reaches every page, entity and action. Remains instant, no animation.
- Shortcuts: number keys for tabs, `/` for search, `f` to favourite, `p` to pin, `[` and `]` for patch, with a cheat sheet on `?`.

### Theme and density (Q7)

- Light theme from the same tokens. Compact density reduces row height and padding for laptops.

### Backup (Q8)

- One file: settings (without the key), favourites, hidden comps, overrides, saved teams, notes, tier lists, followed players, capture rows. Raw matches are excluded because the crawler can refetch them.
- Import previews what will be added or replaced.

### Errors (Q9)

- One pattern: what happened, what to do, a copy button for the log path. No apologies, no vague wording.
- The engine log is viewable in the app with level filters.

### Tests (Q10)

| Layer | What | Runs |
| --- | --- | --- |
| Rust unit | Limiter, dating, clustering, index, capture recognition | Every commit |
| Parity and golden API | Fixture through the engine, responses compared with saved files | Every commit |
| Screenshot | 12 screens by 4 stress modes | Every commit that touches UI |
| Soak | 24-hour crawl | Before each phase gate |
| Manual | 10 games with the overlay | Gates 2 and 3 |

The 12 screens: Comps, comp detail, Units, unit detail, Items, Traits, Explorer, Builder, Profile, match detail, Settings, pinned overlay panel.

## Continuous slice

Not everything waits for week 12:

| When | What |
| --- | --- |
| Week 1 | Test layers set up (Q10) |
| Every Sunday | Real-data checklist (Q4), budgets check (Q5) |
| Each new page | Empty, loading, error states; stress modes; keyboard path |
| Week 5 | Settings page skeleton, since the desktop app needs it (Q2) |
| Week 12 | First run, identity completion, theme, backup, bug bash |

## Edge cases

| Case | Behaviour |
| --- | --- |
| Offline at first run | Explains that the key test and static data need a connection; offers demo data |
| Backup from a newer app version | Refused with the version named |
| Keychain unavailable | Falls back to an encrypted file with a notice |
| Theme switched while overlay is open | Panels update live |

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| Week 1 | Test harness and screenshot suite | Q10 |
| Week 5 | Settings skeleton | Q2 |
| 1 | First-run flow | Q1 |
| 2 | Settings complete; errors and log viewer | Q2, Q9 |
| 3 | Identity completion | Q3 |
| 4 | Keyboard, theme, density | Q6, Q7 |
| 5 | Backup; performance pass | Q8, Q5 |
| 6 | Real-data pass and bug bash | Q4 |

## Open questions

- Screenshots of the app on your real data are still needed to start Q4.
- Whether the Quiver and Higgsfield tools get connected, or you run the prompts in `identity/BRIEF.md` yourself.
