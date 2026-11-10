import { useState } from 'react'
import { count, num, useApi, useFilters, type Entity, type Rates, type Scope } from '../api'
import { Empty, GradeChip, Icon, rateCols, Search, Segmented, Status, Table } from '../parts'
import { gradeByPlace } from '../tiers'

type Level = { level: number; units: number } & Rates
type TraitRow = Entity & Rates & { levels: Level[] }
type LevelRow = Entity & Rates & { units: number }

export function Traits() {
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
          { key: 'name', label: 'Trait', sort: (r) => r.name, render: (r) => <span className="name"><Icon e={r} size={26} plain /><span>{r.name}</span></span> },
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
          { key: 'name', label: 'Trait', sort: (r) => `${r.name} ${String(r.units).padStart(2, '0')}`, render: (r) => <span className="name"><Icon e={r} size={26} plain /><b className="units-count">{r.units}</b><span>{r.name}</span></span> },
          { key: 'grade', label: 'Tier', render: (r) => <GradeChip grade={gradeByPlace(r.avgPlace)} />, width: '60px' },
          ...rateCols<LevelRow>(),
        ]} />
      )}
    </>
  )
}
