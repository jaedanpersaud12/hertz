import { useEffect, useMemo, useRef, useState } from 'react'
import { send, useApi, type Entity } from '../api'
import { CollapsibleBanner } from '../interior/collapsible-banner'
import { ContextMenu } from '../interior/context-menu'
import { FilterGrid } from '../interior/filter-grid'
import { HoldToConfirm } from '../interior/hold-to-confirm'
import { LoadingButton } from '../interior/loading-button'
import { Icon, Search, Status } from '../parts'
import { TraitGem } from '../art'
import type { StaticData } from './Explore'

const ROWS = 4
const COLS = 7
const STORE = 'tft-local-board'
export type Board = Record<number, string> // hex number (0 to 27, front row first) -> unit id

const load = (): Board => {
  try { return JSON.parse(localStorage.getItem(STORE) ?? '{}') } catch { return {} }
}
/** Lets My lists hand a saved team to the builder. */
export const openTeam = (board: Board) => {
  try { localStorage.setItem(STORE, JSON.stringify(board)) } catch { /* private mode: the team just won't persist */ }
  location.hash = '/builder'
}

/** Count each trait once per distinct unit, then find the highest breakpoint reached. */
export function activeTraits(board: Board, data: StaticData) {
  const units = new Map(data.units.map((u) => [u.id, u]))
  const counts = new Map<string, number>()
  for (const id of new Set(Object.values(board))) {
    for (const t of units.get(id)?.traits ?? []) counts.set(t, (counts.get(t) ?? 0) + 1)
  }
  return [...counts].map(([name, count]) => {
    const trait = data.traits.find((t) => t.name === name || t.id === name)
    const points = trait?.breakpoints ?? []
    const reached = [...points].reverse().find((b) => b.units <= count)
    const next = points.find((b) => b.units > count)
    return { name, count, trait, style: reached?.style ?? 0, next: next?.units ?? null }
  }).sort((a, b) => b.style - a.style || b.count - a.count || a.name.localeCompare(b.name))
}

/** A unit's picture inside a hex, falling back to its name when there is no picture. */
function HexFace({ unit, fallback }: { unit?: Entity; fallback: string }) {
  const [broken, setBroken] = useState(false)
  const name = unit?.name ?? fallback.replace(/^TFT\d*_/, '')
  if (unit?.icon && !broken) return <img src={unit.icon} alt={name} onError={() => setBroken(true)} />
  return <span>{name.length > 8 ? name.slice(0, 7) + '…' : name}</span>
}

export function Builder() {
  const { data, error, loading } = useApi<StaticData>('/api/static')
  const [board, setBoard] = useState<Board>(load)
  const [search, setSearch] = useState('')
  const [name, setName] = useState('')
  // The unit pool fits as many columns as its width allows, instead of squeezing a fixed twelve.
  const poolRef = useRef<HTMLElement>(null)
  const [columns, setColumns] = useState(12)
  useEffect(() => {
    const el = poolRef.current
    if (!el) return
    const fit = () => setColumns(Math.max(3, Math.min(12, Math.floor(el.clientWidth / 92))))
    fit()
    const ro = new ResizeObserver(fit)
    ro.observe(el)
    return () => ro.disconnect()
  }, [data])

  useEffect(() => { try { localStorage.setItem(STORE, JSON.stringify(board)) } catch { /* ignore */ } }, [board])

  const units = useMemo(() => new Map((data?.units ?? []).map((u) => [u.id, u])), [data])
  const traits = useMemo(() => (data ? activeTraits(board, data) : []), [board, data])
  const pool = useMemo(() => {
    const q = search.trim().toLowerCase()
    return (data?.units ?? []).filter((u) => !q || u.name.toLowerCase().includes(q) || (u.traits ?? []).some((t) => t.toLowerCase().includes(q)))
      .sort((a, b) => (a.cost ?? 0) - (b.cost ?? 0) || a.name.localeCompare(b.name))
  }, [data, search])
  const costFilters = useMemo(() => [
    { id: 'all', label: 'All', match: () => true },
    ...[...new Set((data?.units ?? []).map((u) => u.cost ?? 0))].filter(Boolean).sort()
      .map((c) => ({ id: String(c), label: `${c} cost`, match: (u: Entity) => u.cost === c })),
  ], [data])
  if (error || !data) return <Status error={error} loading={loading} />

  const place = (id: string, hex?: number) => setBoard((b) => {
    const target = hex ?? Array.from({ length: ROWS * COLS }, (_, i) => i).find((i) => !b[i])
    return target === undefined ? b : { ...b, [target]: id }
  })
  const move = (from: number, to: number) => setBoard((b) => {
    const next = { ...b }
    const [a, z] = [b[from], b[to]]
    if (z) next[from] = z; else delete next[from]
    next[to] = a
    return next
  })
  const remove = (hex: number) => setBoard((b) => { const next = { ...b }; delete next[hex]; return next })

  const drop = (e: React.DragEvent, hex: number) => {
    e.preventDefault()
    const from = e.dataTransfer.getData('hex')
    const id = e.dataTransfer.getData('unit')
    if (from) move(Number(from), hex)
    else if (id) place(id, hex)
  }
  const save = async () => {
    await send('/api/teams', { name: name.trim() || 'Untitled team', board })
    setName('')
  }
  const count = Object.keys(board).length

  return (
    <div className="builder">
      {!data.loaded ? (
        <div className="mb-5">
          <CollapsibleBanner title="Unit costs and traits are not downloaded yet"
            description={<>Traits cannot be counted until they are. Run <code>npm run static</code> in the project folder.</>} />
        </div>
      ) : null}
      <div className="builder-main">
        <section className="board-col">
          <div className="hexes" aria-label="Board">
            {Array.from({ length: ROWS }, (_, r) => (
              <div key={r} className="hex-row">
                {Array.from({ length: COLS }, (_, c) => {
                  const hex = r * COLS + c
                  const u = units.get(board[hex])
                  const cell = (
                    <div key={hex} className={`hex${board[hex] ? ` filled cost-bg-${u?.cost ?? 0}` : ''}`}
                      onDragOver={(e) => e.preventDefault()} onDrop={(e) => drop(e, hex)}>
                      {board[hex] ? (
                        <button className="hex-unit" draggable title={`${u?.name ?? board[hex]}: click to remove, drag to move, right-click for more`}
                          onDragStart={(e) => e.dataTransfer.setData('hex', String(hex))} onClick={() => remove(hex)}>
                          <HexFace unit={u} fallback={board[hex]} />
                        </button>
                      ) : null}
                    </div>
                  )
                  return board[hex] ? (
                    <ContextMenu key={hex} label={`Actions for ${u?.name ?? 'this unit'}`} width={190}
                      items={[{ id: 'open', label: `Open ${u?.name ?? 'unit'} page` }, { id: 'remove', label: 'Remove from board' }]}
                      onSelect={(id) => (id === 'remove' ? remove(hex) : (location.hash = `/units/${encodeURIComponent(board[hex])}`))}>
                      {cell}
                    </ContextMenu>
                  ) : cell
                })}
              </div>
            ))}
          </div>
          <p className="muted small board-note">{count} unit{count === 1 ? '' : 's'} on the board. Top row is the front line. Click a unit below to add it, drag to move, click on the board to remove.</p>

          <div className="toolbar">
            <input className="field min-w-0 flex-1" value={name} onChange={(e) => setName(e.target.value)} placeholder="Team name" aria-label="Team name" maxLength={60} />
            <LoadingButton className="ibtn ibtn-primary" onAction={save} disabled={!count} pendingLabel="Saving" successLabel="Saved to My lists">Save team</LoadingButton>
            <HoldToConfirm className="ibtn" onConfirm={() => setBoard({})} disabled={!count} confirmLabel="Board cleared">Hold to clear board</HoldToConfirm>
          </div>
        </section>

        <aside className="builder-traits card">
          <h3>Traits</h3>
          {traits.length ? (
            <ul>
              {traits.map((t) => (
                <li key={t.name} className={t.style ? '' : 'inactive'}>
                  <TraitGem count={t.count} style={t.style} size={26} />
                  <span>{t.trait?.name ?? t.name}</span>
                  <span className="muted small">{t.next ? `next at ${t.next}` : t.style ? 'max' : ''}</span>
                </li>
              ))}
            </ul>
          ) : <p className="muted small">Add units to see which traits switch on.</p>}
        </aside>
      </div>

      <section className="pool" ref={poolRef}>
        <div className="toolbar"><Search value={search} onChange={setSearch} placeholder="Search units or traits" /></div>
        <FilterGrid label="Units" items={pool} filters={costFilters} getKey={(u) => u.id} columns={columns} rowHeight={88} gap={6} maxRows={4}
          emptyLabel="No unit matches"
          renderItem={(u) => (
            <button className="pool-unit" draggable title={[u.name, ...(u.traits ?? [])].join(', ')}
              onDragStart={(e) => e.dataTransfer.setData('unit', u.id)} onClick={() => place(u.id)}>
              <Icon e={u} size={40} />
              <span>{u.name}</span>
            </button>
          )} />
      </section>
    </div>
  )
}
