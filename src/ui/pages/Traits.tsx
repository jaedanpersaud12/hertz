import { CaretLeft } from '@phosphor-icons/react'
import { useEffect, useState } from 'react'
import { count, num, pct, useApi, useFilters, type Entity, type Rates, type Scope } from '../api'
import { Empty, Fact, GradeChip, Icon, PlaceRail, rateCols, Search, Segmented, Status, Table } from '../parts'
import { gradeByPlace } from '../tiers'

type Level = { level: number; units: number } & Rates
type TraitRow = Entity & Rates & { levels: Level[] }
type LevelRow = Entity & Rates & { units: number }

const traitLink = (r: Entity) => `#/traits/${encodeURIComponent(r.id)}`

/** The trait in the address (#/traits/:id), if any. The app routes #/traits/* here. */
function useTraitId() {
  const read = () => {
    const sub = location.hash.replace(/^#\/?/, '').split('/')[1]
    return sub ? decodeURIComponent(sub) : null
  }
  const [id, setId] = useState(read)
  useEffect(() => {
    const on = () => setId(read())
    addEventListener('hashchange', on)
    return () => removeEventListener('hashchange', on)
  }, [])
  return id
}

export function Traits() {
  const id = useTraitId()
  return id ? <Trait id={id} /> : <TraitList />
}

function TraitList() {
  const f = useFilters()
  const { data, error, loading } = useApi<Scope & { rows: TraitRow[] }>('/api/traits', f)
  const [search, setSearch] = useState('')
  const [view, setView] = useState<'overall' | 'level'>('overall')
  if (error || !data) return <Status error={error} loading={loading} />
  const rows = data.rows.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()))

  // One row per trait and breakpoint, e.g. "6 Vanguard".
  const byLevel: LevelRow[] = rows.flatMap((r) =>
    r.levels.filter((l) => l.games >= f.min).map((l) => ({ ...r, ...l, id: `${r.id}|${l.level}`, name: r.name, units: l.units })))

  return (
    <>
      <div className="toolbar">
        <Search value={search} onChange={setSearch} placeholder="Search traits" />
        <Segmented label="View" value={view} onChange={setView} options={[{ value: 'overall', label: 'Overall' }, { value: 'level', label: 'By breakpoint' }]} />
        <span className="toolbar-note">{count(view === 'overall' ? rows.length : byLevel.length, 'row')}</span>
      </div>
      {!rows.length ? <Empty scope={data} what="No trait" /> : view === 'overall' ? (
        <Table key="overall" label="Traits" rows={rows} initial="place" cols={[
          { key: 'name', label: 'Trait', sort: (r) => r.name, render: (r) => <a className="name" href={traitLink(r)}><Icon e={r} size={26} plain /><span>{r.name}</span></a> },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByPlace(r.avgPlace)} />, width: '60px' },
          ...rateCols<TraitRow>(),
          { key: 'levels', label: 'Breakpoints played', width: '200px', render: (r) => (
            <span className="levels">
              {r.levels.map((l, n) => (
                <span key={l.level} className={`level level-${Math.min(n, 3)}`} style={{ flexGrow: l.games }}
                  title={`${l.units} units: ${num(l.games)} games, avg place ${l.avgPlace.toFixed(2)}`}>{l.units}</span>
              ))}
            </span>
          ) },
        ]} />
      ) : (
        <Table key="level" label="Traits by breakpoint" rows={byLevel} initial="place" cols={[
          { key: 'name', label: 'Trait', sort: (r) => `${r.name} ${String(r.units).padStart(2, '0')}`, render: (r) => <a className="name" href={traitLink({ ...r, id: r.id.split('|')[0] })}><Icon e={r} size={26} plain /><b className="units-count">{r.units}</b><span>{r.name}</span></a> },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByPlace(r.avgPlace)} />, width: '60px' },
          ...rateCols<LevelRow>(),
        ]} />
      )}
    </>
  )
}

type UnitSet = { units: Entity[]; games: number; share: number; avgPlace: number }
type Breakpoint = Rates & { id: string; level: number; units: number; set: UnitSet | null }
type TraitDetail = Scope & { trait: Entity & Partial<Rates>; levels: Breakpoint[]; setData: boolean }

function UsualUnits({ set, setData }: { set: UnitSet | null; setData: boolean }) {
  if (!set) return <span className="muted small">{setData ? 'No set of units repeats often enough yet.' : 'Needs set data.'}</span>
  return (
    <span className="usual" title={`${num(set.games)} games, avg place ${set.avgPlace.toFixed(2)}`}>
      <span className="mini-board flex-wrap gap-1">{set.units.map((u) => <a key={u.id} href={`#/units/${encodeURIComponent(u.id)}`} aria-label={u.name}><Icon e={u} size={26} /></a>)}</span>
      <span className="muted small">{pct(set.share, 0)} of boards</span>
    </span>
  )
}

/** U8: one row per active breakpoint with avg place, pick rate and the most common units at that count. A trait
 * with one breakpoint shows it as a single card. */
function Trait({ id }: { id: string }) {
  const f = useFilters()
  const { data, error, loading } = useApi<TraitDetail>(`/api/traits/${encodeURIComponent(id)}`, f)
  if (error || !data) return <><a className="back" href="#/traits"><CaretLeft size={12} /> All traits</a><Status error={error} loading={loading} /></>
  const t = data.trait
  const thin = (r: Rates) => r.games < f.min
  const one = data.levels.length === 1 ? data.levels[0] : null
  return (
    <>
      <a className="back" href="#/traits"><CaretLeft size={12} /> All traits</a>
      <header className="detail-head">
        <Icon e={t} size={56} plain />
        <div className="detail-title">
          <h1>{t.name}</h1>
          <p className="muted"><span>{data.levels.length === 1 ? '1 breakpoint played' : `${data.levels.length} breakpoints played`}</span></p>
        </div>
        {t.games ? (
          <dl className="facts">
            <Fact label="Avg place">{t.avgPlace!.toFixed(2)}</Fact>
            <Fact label="Pick rate">{pct(t.frequency!)}</Fact>
            <Fact label="Games">{num(t.games)}</Fact>
          </dl>
        ) : null}
      </header>
      {!data.levels.length ? <Empty scope={data} what="No breakpoint of this trait" /> : one ? (
        <section className={`card breakpoint-one${thin(one) ? ' thin' : ''}`}>
          <h3>{one.units} {t.name}</h3>
          <dl className="kv">
            <div><dt>Avg place</dt><dd>{one.avgPlace.toFixed(2)}</dd></div>
            <div><dt>Pick rate</dt><dd>{pct(one.frequency)}</dd></div>
            <div><dt>Games</dt><dd>{num(one.games)}</dd></div>
          </dl>
          <h3 className="mt-3">Usual units</h3>
          <UsualUnits set={one.set} setData={data.setData} />
        </section>
      ) : (
        <Table label={`${t.name} by breakpoint`} rows={data.levels} initial="level" rowHeight={60} cols={[
          { key: 'level', label: 'Breakpoint', sort: (r) => r.level, render: (r) => <span className={thin(r) ? 'thin' : undefined}><b className="units-count">{r.units}</b> {t.name}</span>, width: 'minmax(150px, 1fr)' },
          { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => <span className={thin(r) ? 'thin' : undefined}><PlaceRail value={r.avgPlace} /></span>, width: '150px' },
          { key: 'pick', label: 'Pick rate', sort: (r) => r.frequency, render: (r) => <span className={thin(r) ? 'thin' : undefined}>{pct(r.frequency)}</span>, align: 'right', width: '100px' },
          { key: 'games', label: 'Games', sort: (r) => r.games, render: (r) => <span className={thin(r) ? 'thin' : undefined}>{num(r.games)}</span>, align: 'right', width: '100px' },
          { key: 'set', label: 'Usual units', render: (r) => <UsualUnits set={r.set} setData={data.setData} />, width: 'minmax(260px, 2.4fr)' },
        ]} />
      )}
      {data.levels.length && !data.setData ? <p className="muted small mt-3">Which units make up each breakpoint comes from set data. Run <code>npm run refresh</code>, then <code>npm run stats</code>.</p> : null}
    </>
  )
}
