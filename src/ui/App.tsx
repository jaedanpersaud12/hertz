import { ArrowsClockwise, CaretRight, MagnifyingGlass, SidebarSimple, X } from '@phosphor-icons/react'
import { useEffect, useMemo, useState } from 'react'
import { ago, FiltersCtx, MarksCtx, num, useApi, useMarks, useMarksState, type Comp, type Filters, type Meta, type Scope } from './api'
import { IdentitySheet, LogoMark, PlaceMedal } from './art'
import { DevData } from './DevData'
import { CommandPalette, type CommandItem } from './interior/command-palette'
import { Drawer } from './interior/drawer'
import { SkeletonSwap } from './interior/skeleton-swap'
import { TooltipGroup } from './interior/tooltip-group'
import { Button, IconButton, PlaceRail, Select, Status, TraitChip, UnitChip } from './parts'
import { Builder } from './pages/Builder'
import { Comps } from './pages/Comps'
import { Data } from './pages/Data'
import { Explore, type StaticData } from './pages/Explore'
import { History } from './pages/History'
import { Item, Items } from './pages/Items'
import { Lists } from './pages/Lists'
import { Traits } from './pages/Traits'
import { Unit, Units } from './pages/Units'

const NAV = [
  ['comps', 'Comps'], ['units', 'Units'], ['items', 'Items'], ['traits', 'Traits'],
  ['explore', 'Explorer'], ['builder', 'Team builder'], ['lists', 'My lists'],
  ['history', 'Match history'], ['data', 'Data'],
] as const
const TITLES: Record<string, string> = Object.fromEntries(NAV)
const SUBTITLES: Record<string, string> = {
  comps: 'Comps grouped from real final boards, ranked by how they place.',
  units: 'How every unit places, and what to build on it.',
  items: 'Which items help, measured against each unit’s own average.',
  traits: 'Every trait, overall or at each breakpoint.',
  explore: 'Ask the stored boards a question.',
  builder: 'Plan a board and watch the traits switch on.',
  lists: 'Your favourite comps, saved teams and hidden comps.',
  history: 'Your recent games, with every lobby.',
  data: 'What is stored on this machine, and how to refresh it.',
}

function useHash() {
  const read = () => location.hash.replace(/^#\/?/, '') || 'comps'
  const [hash, setHash] = useState(read)
  useEffect(() => {
    const on = () => { setHash(read()); window.scrollTo(0, 0) }
    addEventListener('hashchange', on)
    return () => removeEventListener('hashchange', on)
  }, [])
  return hash
}

function useMedia(query: string) {
  const [match, setMatch] = useState(() => matchMedia(query).matches)
  useEffect(() => {
    const m = matchMedia(query)
    const on = () => setMatch(m.matches)
    m.addEventListener('change', on)
    return () => m.removeEventListener('change', on)
  }, [query])
  return match
}

const PALETTE_HINT = 'Search pages, comps, units and items'
const STORE = 'tft-local-filters'
const loadFilters = (): Filters => {
  // The patch is not remembered: a new session should open on the newest one.
  try { return { bucket: 'ALL', window: '', min: 10, ...JSON.parse(localStorage.getItem(STORE) ?? '{}'), patch: '' } }
  catch { return { patch: '', bucket: 'ALL', window: '', min: 10 } }
}

// ---- Your panel: account, pinned comp, what this view covers ----------------------
type Me = {
  configured: boolean; error?: string | null; name?: string; tag?: string; tier?: string | null; division?: string | null
  lp?: number | null; recent?: number[]; avgPlace?: number | null; fetchedAt?: number
}
const title = (s: string) => s.charAt(0) + s.slice(1).toLowerCase()

function Summoner() {
  const { data, reload } = useApi<Me>('/api/me')
  const [busy, setBusy] = useState(false)
  const refresh = async () => { setBusy(true); await fetch('/api/me/refresh', { method: 'POST' }).catch(() => {}); setBusy(false); reload() }
  const apex = ['MASTER', 'GRANDMASTER', 'CHALLENGER'].includes(data?.tier ?? '')
  return (
    <section className="side-card">
      <SkeletonSwap ready={!!data} reserve={data && !data.name ? 62 : data?.error ? 190 : 112} lines={4} label="Your account" className="grid-cols-[minmax(0,1fr)]">
        {!data ? null : !data.configured ? (
          <>
            <h3>Your account</h3>
            <p className="muted small mt-1">Add <code>RIOT_ID</code> and <code>MY_PLATFORM</code> to <code>.env</code> to see your rank and recent games. <a href="#/history">How</a></p>
          </>
        ) : !data.name ? (
          <><h3>Your account</h3><p className="error small">{data.error}</p></>
        ) : (
          <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2">
            <div className="side-head">
              <h3 title={`${data.name}#${data.tag}`}><span className="riot-name">{data.name}</span><span className="muted font-normal">#{data.tag}</span></h3>
              <IconButton label={busy ? 'Checking Riot' : `Updated ${ago(data.fetchedAt)}. Check again`} onClick={refresh} side="bottom"><ArrowsClockwise size={15} /></IconButton>
            </div>
            <p className="rank">{data.tier ? <>{title(data.tier)}{apex ? '' : ` ${data.division}`} <span className="muted font-normal">{data.lp} LP</span></> : <span className="muted">Unranked</span>}</p>
            {data.recent?.length ? (
              <>
                <div className="recent">{data.recent.map((p, n) => <PlaceMedal key={n} place={p} size={22} />)}</div>
                <p className="muted small">Average place {data.avgPlace!.toFixed(2)} over the last {data.recent.length} games</p>
              </>
            ) : <p className="muted small">No recent games found.</p>}
            {data.error ? <p className="error small">{data.error}</p> : null}
          </div>
        )}
      </SkeletonSwap>
    </section>
  )
}

function Pinned({ f }: { f: Filters }) {
  const marks = useMarks()
  const { data } = useApi<Scope & { rows: Comp[] }>(marks.pinned ? '/api/comps' : null, { ...f, min: 1 })
  const comp = data?.rows.find((c) => c.id === marks.pinned)
  return (
    <section className="side-card">
      <div className="side-head">
        <h3>Pinned comp</h3>
        {marks.pinned ? <IconButton label="Unpin" onClick={() => marks.pin(null)} side="bottom"><X size={14} /></IconButton> : null}
      </div>
      {!marks.pinned ? <p className="muted small">Press the pin on a comp to keep its board here while you browse.</p>
        : !comp ? <p className="muted small">The pinned comp has no games at the current filters.</p> : (
          <>
            <p className="pinned-name">{comp.name}</p>
            <div className="comp-traits">{comp.traits.slice(0, 6).map((t) => <TraitChip key={t.id} t={t} />)}</div>
            <div className="pinned-units">{comp.units.map((u) => <UnitChip key={u.id} u={u} size={32} />)}</div>
            <div className="pinned-stats"><PlaceRail value={comp.avgPlace} /><span className="muted small">{comp.play.label}</span></div>
          </>
        )}
    </section>
  )
}

function Panel({ f, meta }: { f: Filters; meta: Meta | null }) {
  return (
    <>
      <Summoner />
      <Pinned f={f} />
      {meta ? (
        <section className="side-card quiet">
          <h3>This view</h3>
          <dl className="kv">
            <div><dt>Boards counted</dt><dd>{num(meta.comps)}</dd></div>
            <div><dt>Matches</dt><dd>{num(meta.matches)}</dd></div>
            <div><dt>Newest match</dt><dd>{ago(meta.totals.newest)}</dd></div>
            <div><dt>Stats rebuilt</dt><dd>{ago(meta.builtAt)}</dd></div>
          </dl>
          <a className="side-link" href="#/data">Data and commands <CaretRight size={11} /></a>
        </section>
      ) : null}
    </>
  )
}

// ---- Jump anywhere (Ctrl or Cmd + K) -----------------------------------------------
function useJumpItems(f: Filters): CommandItem[] {
  const statics = useApi<StaticData>('/api/static')
  const comps = useApi<Scope & { rows: Comp[] }>('/api/comps', { ...f, min: 1 })
  return useMemo(() => [
    ...NAV.map(([k, label]) => ({ id: `page:${k}`, label, hint: 'Page' })),
    ...(comps.data?.rows ?? []).map((c) => ({ id: `comp:${c.id}`, label: c.name, hint: 'Comp', keywords: c.units.map((u) => u.name).join(' ') })),
    ...(statics.data?.units ?? []).map((u) => ({ id: `units:${u.id}`, label: u.name, hint: u.cost ? `Unit, ${u.cost} cost` : 'Unit', keywords: (u.traits ?? []).join(' ') })),
    ...(statics.data?.items ?? []).map((i) => ({ id: `items:${i.id}`, label: i.name, hint: 'Item' })),
  ], [statics.data, comps.data])
}

// ---- Shell -------------------------------------------------------------------------
export function App() {
  const hash = useHash()
  const [page, sub] = hash.split('/')
  const id = sub ? decodeURIComponent(sub) : null
  const [f, setF] = useState<Filters>(loadFilters)
  const marks = useMarksState()
  const meta = useApi<Meta>('/api/meta', f)
  const m = meta.data
  const wide = useMedia('(min-width: 1560px)')
  const [drawer, setDrawer] = useState(false)
  const [palette, setPalette] = useState(false)
  const jump = useJumpItems(f)

  useEffect(() => { try { localStorage.setItem(STORE, JSON.stringify(f)) } catch { /* ignore */ } }, [f])
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setPalette((p) => !p) }
    }
    addEventListener('keydown', on)
    return () => removeEventListener('keydown', on)
  }, [])

  // The palette does not take focus by itself when it opens as an overlay, so hand it over once its input exists.
  useEffect(() => {
    if (!palette) return
    const back = document.activeElement as HTMLElement | null
    let tries = 0
    const timer = setInterval(() => {
      const input = document.querySelector<HTMLInputElement>(`input[placeholder="${PALETTE_HINT}"]`)
      if (input) { input.focus(); clearInterval(timer) }
      else if (++tries > 20) clearInterval(timer)
    }, 25)
    return () => { clearInterval(timer); back?.focus?.() }
  }, [palette])

  const go = (item: CommandItem) => {
    const [kind, ...rest] = item.id.split(':')
    const target = rest.join(':')
    setPalette(false)
    if (kind === 'page') location.hash = `/${target}`
    else if (kind === 'comp') { sessionStorage.setItem('tft-local-open-comp', target); location.hash = '/comps'; dispatchEvent(new Event('open-comp')) }
    else location.hash = `/${kind}/${encodeURIComponent(target)}`
  }

  const current = TITLES[page] ? page : page === 'identity' ? '' : 'comps'
  const heading = page === 'identity' ? null : id && page === 'units' ? null : id && page === 'items' ? null : TITLES[current]
  const hasStats = m && m.patches.length > 0
  const isMac = /Mac/i.test(navigator.platform)

  return (
    <FiltersCtx.Provider value={f}>
      <MarksCtx.Provider value={marks}>
        <TooltipGroup className="contents">
          <header className="topbar">
            <div className="topbar-row">
              <a className="brand" href="#/comps">
                <LogoMark size={24} />
                TFT Local
              </a>
              {hasStats ? (
                <div className="scope">
                  <span className="crumb-sep" aria-hidden>/</span>
                  <Select quiet label="Patch" prefix="Patch" value={f.patch || m.patch} onChange={(patch) => setF({ ...f, patch })}
                    options={m.patches.map((p) => ({ value: p, label: p }))} />
                  <span className="crumb-sep" aria-hidden>/</span>
                  <Select quiet label="Rank" value={f.bucket} onChange={(bucket) => setF({ ...f, bucket })}
                    options={m.buckets.map((b) => ({ value: b, label: b === 'ALL' ? 'All ranks' : title(b) }))} />
                  <span className="scope-extra contents">
                    <span className="crumb-sep" aria-hidden>/</span>
                    <Select quiet label="Period" value={f.window} onChange={(window) => setF({ ...f, window })}
                      options={m.windows.map((w) => ({ value: w.key, label: w.label }))} />
                    <span className="crumb-sep" aria-hidden>/</span>
                    <Select quiet label="Minimum games" value={f.min} onChange={(min) => setF({ ...f, min })}
                      options={[1, 10, 50, 200, 1000].map((n) => ({ value: n, label: `${num(n)}+ games`, hint: n === 1 ? 'everything' : undefined }))} />
                  </span>
                </div>
              ) : null}
              <div className="topbar-end">
                <Button className="search-trigger" onClick={() => setPalette(true)} aria-label="Search units, items and comps">
                  <MagnifyingGlass size={14} /> <span>Search</span> <kbd className="kbd">{isMac ? '⌘K' : 'Ctrl K'}</kbd>
                </Button>
                {!wide ? <Button square aria-label="Open your panel" onClick={() => setDrawer(true)}><SidebarSimple size={16} mirrored /></Button> : null}
              </div>
            </div>
            <nav className="tabnav" aria-label="Sections">
              {NAV.map(([k, label]) => (
                <a key={k} href={`#/${k}`} aria-current={current === k ? 'page' : undefined}>
                  {label}
                  {/* No slide: this is core navigation, used all day. The line just moves. */}
                  {current === k ? <span className="tabnav-line" /> : null}
                </a>
              ))}
            </nav>
          </header>

          <div className={`layout${wide ? '' : ' solo'}`}>
            <main>
              {heading ? (
                <header className="page-head">
                  <div>
                    <h1>{heading}</h1>
                    <p className="page-sub">{SUBTITLES[current]}</p>
                  </div>
                </header>
              ) : null}
              <Status error={meta.error} loading={false} />
              {page === 'units' ? (id ? <Unit id={id} /> : <Units />)
                : page === 'items' ? (id ? <Item id={id} /> : <Items />)
                : page === 'traits' ? <Traits />
                : page === 'explore' ? <Explore />
                : page === 'builder' ? <Builder />
                : page === 'lists' ? <Lists />
                : page === 'history' ? <History />
                : page === 'identity' ? <IdentitySheet />
                : page === 'data' ? (m ? <Data meta={m} /> : null)
                : <Comps />}
            </main>
            {wide ? <aside className="side" aria-label="Your panel"><Panel f={f} meta={m} /></aside> : null}
          </div>

          <Drawer open={drawer && !wide} onOpenChange={setDrawer} title="Your panel" side="right" width={340}>
            <div className="drawer-body"><Panel f={f} meta={m} /></div>
          </Drawer>
          <CommandPalette open={palette} items={jump} onSelect={go} onDismiss={() => setPalette(false)}
            placeholder={PALETTE_HINT} emptyLabel="Nothing matches" label="Search" maxRows={8} />
          {import.meta.env.DEV ? <DevData /> : null}
        </TooltipGroup>
      </MarksCtx.Provider>
    </FiltersCtx.Provider>
  )
}
