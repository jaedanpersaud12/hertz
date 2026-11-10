# PRD 6: Desktop app and in-game overlay

> **Policy change, 10 Nov 2026:** in-game and loading-screen scouting (P9, P10, the Scouting panel) is dropped. Riot's TFT policy bans scouting during play. P9 is now a post-game lobby review; P10 is cancelled. See "Policy check" in 00-overview.md.

Weeks 5 to 8 · stories D1 to D13 · depends on PRD 1 (engine as a library) · PRD 7 and the scouting half of PRD 8 depend on this.

TFT Local becomes an installed app: the engine lives in the tray, and a set of small windows draws over the game. The bar is the reason the project exists: it must be faster and steadier than MetaTFT's overlay.

## Context

MetaTFT's overlay runs on a general overlay platform, loads its data over the network, and injects into the game's rendering. Each of those is a source of the lag and bugs you described. This design avoids all three: data is on disk, windows are ordinary operating-system windows that happen to be transparent and on top, and nothing touches the game process.

Think of sticky notes on the monitor glass rather than paint on the picture. The game never knows they are there.

## Goals and measures

| Goal | Measure | Target |
| --- | --- | --- |
| Instant | Hotkey press to panel visible | Under 100 ms |
| Invisible cost | Game frame rate with overlay shown against hidden, same scene | Within 1% |
| Idle cost | App CPU while in game with panels visible and idle | Under 1% |
| Never in the way | Focus changes caused by the overlay in 1,000 show and hide cycles | 0 |
| No setup | Terminal commands from installer to first comp list | 0 |
| Stable | Crashes in 50 games | 0 |

## Non-goals

Exclusive fullscreen. Any input sent to the game. Reading game memory. Windows and Mac at the same time: one platform ships first, chosen by where you play.

## Requirements

### Shell

1. Tauri 2 app. The main window is the existing React app, served from the bundle and talking to the engine through Tauri commands or the local API on a loopback port.
2. The engine starts with the app and keeps running when the main window is closed. Quit is explicit, from the tray.
3. Tray menu: crawl status with matches per hour, pause and resume, open, settings, quit.
4. Optional launch at login.
5. Data location is the operating system's app data folder. First run offers to import an existing `data/tft.db`.
6. Secrets (the API key) go in the operating system keychain, not a file.

### Game detection (D3)

- State machine: `idle` → `client open` → `in lobby` → `in game` → `post game` → `idle`.
- Sources in order: the League client's local API, then the game process appearing, then the game window gaining focus.
- Overlay windows exist only in `in game`. `post game` triggers the match refresh and summary (PRD 8).

### Overlay windows

- One window per panel: transparent, borderless, always on top, non-activating, skipped by the taskbar and by alt-tab.
- Two modes per panel. **Locked:** click-through, the game receives all input. **Interactive:** while the overlay modifier key is held or the panel is toggled open, the panel takes clicks.
- Panels follow the game window's position and scale, and hide when the game loses focus to anything except the overlay itself.
- Positions, sizes and opacity persist per display resolution.
- All panels render the app's own components and tokens. No entrance animation on hotkey panels.

### Panels

| Panel | Default | Content | Story |
| --- | --- | --- | --- |
| Pinned comp | Shown, locked, right edge | Board for the current level plan step, carry items, levelling note, your notes | D4 |
| Comp list | Hotkey, hold | Tier list filtered by typed units; Enter pins a comp | D5 |
| Item sheet | Hotkey, toggle | Component grid; hover shows what it builds and best holders in the pinned comp | D6 |
| Lookup | Hotkey, toggle | Search any unit, item or trait; compact result card | D7 |
| Scouting | Auto at load-in, 20 seconds | Opponent ranks and recent comps (PRD 8) | P9 |
| Capture status | Dot, corner | Reading, partial, off (PRD 7) | L14 |

### Hotkeys

- Global, registered only while `in game`, released otherwise so they never clash outside TFT.
- Rebindable in Settings. A conflict with another app is reported, not silently lost.

### Data access

- Overlay windows read through a dedicated read-only connection. WAL mode means a stats rebuild never blocks them (D11).
- Panel payloads are precomputed when a comp is pinned, so showing a panel is rendering, not querying.

### Updates (D13)

- Tauri updater against a private release feed (a private repository's releases is enough). Signed builds; update applies on next start, never mid-game.

## Platform notes

|  | Windows | macOS |
| --- | --- | --- |
| Window over borderless game | Topmost layered window | Panel at a high window level; game must be windowed fullscreen |
| Click-through | Supported by window flags | Supported by ignoring mouse events |
| Anti-cheat | Vanguard runs at kernel level; ordinary topmost windows are how other overlays in this style work, to be confirmed by the spike | No kernel anti-cheat on the Mac client at last check; confirm in the spike |
| Global hotkeys | Standard | Needs accessibility or input-monitoring permission |
| Signing | Unsigned builds trigger SmartScreen; fine for private use | Unsigned builds need a right-click open; notarisation needs a paid developer account |

Everything in this table is to be verified in the week 5 spike; treat it as expectations, not facts.

## Spike: overlay feasibility (week 5, day 1)

1. Bare Tauri app with one transparent, topmost, click-through window showing a clock.
2. Launch TFT in borderless mode, play a normal game.
3. Record: does the window stay on top, does focus ever move, frame rate with and without, any anti-cheat warning.
4. Toggle interactive mode with a hotkey 100 times.
5. Pass means all four are clean. Fail triggers the fallback: the same panels served to a second monitor or phone browser on the local network.

## Edge cases

| Case | Behaviour |
| --- | --- |
| Game in exclusive fullscreen | One-time notice explaining borderless mode; overlay stays hidden |
| Two monitors, game moved | Panels follow the game window |
| Resolution or UI scale changes mid-session | Panels re-anchor proportionally |
| Alt-tab during a game | Panels hide with the game and return with it |
| Engine crashes | Supervisor restarts it; overlay shows last data with a stale marker |
| App updated while in game | Deferred until `idle` |
| Streaming or screen recording | Setting to exclude panels from capture where the platform allows |

## Test plan

- **Automated:** focus test (1,000 cycles) with a dummy fullscreen window standing in for the game; state machine unit tests with recorded client API responses.
- **Performance:** frame-time capture in a fixed scene, overlay on and off.
- **Manual gate:** 10 ranked games across at least 2 sessions; log any crash, focus steal or misplaced panel.
- **Clean machine:** install on a machine that has never had the project, with no developer tools.

## Build order

| Session | Builds | Stories |
| --- | --- | --- |
| 1 | Feasibility spike |  |
| 2 | Tauri shell around the React app; engine as library; tray | D1, D2 |
| 3 | Game detection state machine | D3 |
| 4 | Overlay window manager: create, anchor, lock, persist | D8, D9 |
| 5 | Pinned comp panel | D4 |
| 6 | Comp list and item sheet panels, hotkeys | D5, D6 |
| 7 | Lookup, compact mode, panel switches | D7, D10, D12 |
| 8 | Read-only data path, soak under rebuild | D11 |
| 9 | Installer, updater, clean-machine test | D1, D13 |

## Open questions

- Which platform ships first. Blocks session 1.
- Hold-to-show or toggle as the default for the comp list.
- Whether the main app window and overlay share one webview process (less memory) or separate ones (isolation).
