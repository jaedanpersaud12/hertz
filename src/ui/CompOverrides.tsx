/**
 * Rename a comp or merge it into another (PRD 2 C14). Both are kept on this computer and applied again after
 * every stats rebuild. A rename shows at once; a merge shows after the next stats build.
 */
import { useEffect, useState } from 'react'
import { useApi, useFilters, type Comp, type Scope } from './api'
import { Button, Select, Status } from './parts'

type Override = { id: string; name: string | null; mergeInto: string | null }
type Answer = { override: Override | null; note?: string; error?: string }

export function CompOverrides({ comp }: { comp: Comp }) {
  const f = useFilters()
  const path = `/api/comps/${encodeURIComponent(comp.id)}`
  const current = useApi<Answer>(`${path}/override`)
  const list = useApi<Scope & { rows: Comp[] }>('/api/comps', f)
  const [name, setName] = useState('')
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const saved = current.data?.override ?? null

  useEffect(() => { setName(saved?.name ?? '') }, [saved?.name])

  const act = async (sub: string, body: unknown, method = 'POST') => {
    setBusy(true)
    try {
      const r = await fetch(`${path}/${sub}`, { method, headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) })
      const a: Answer = await r.json().catch(() => ({ override: null, error: 'Could not reach the engine. Check that Hertz is running, then try again.' }))
      setNote(a.error ?? a.note ?? null)
      current.reload()
    } catch {
      setNote('Could not reach the engine. Check that Hertz is running, then try again.')
    }
    setBusy(false)
  }

  if (current.error || !current.data) return <section className="comp-overrides"><h4>Your name and merge</h4><Status error={current.error} loading={current.loading} /></section>

  const others = (list.data?.rows ?? []).filter((c) => c.id !== comp.id)
  const options = [{ value: '', label: 'Not merged' }, ...others.map((c) => ({ value: c.id, label: c.name }))]
  if (saved?.mergeInto && !options.some((o) => o.value === saved.mergeInto)) options.push({ value: saved.mergeInto, label: 'A comp not listed here' })

  return (
    <section className="comp-overrides">
      <h4>Your name and merge</h4>
      <p className="muted small">Kept on this computer and applied again after every stats rebuild.</p>
      <form className="flex gap-2 items-center mt-2" onSubmit={(e) => { e.preventDefault(); act('rename', { name }) }}>
        <input className="field" value={name} maxLength={60} placeholder={comp.name} aria-label="Comp name" onChange={(e) => setName(e.target.value)} />
        <Button size="sm" disabled={busy || name.trim() === (saved?.name ?? '')} onClick={() => act('rename', { name })}>Save name</Button>
      </form>
      <div className="flex gap-2 items-center mt-2">
        <Select label="Merge into" prefix="Merge into" value={saved?.mergeInto ?? ''} options={options}
          onChange={(into) => act('merge', { into: into || null })} />
        {saved ? <Button size="sm" variant="ghost" disabled={busy} onClick={() => act('override', undefined, 'DELETE')}>Undo both</Button> : null}
      </div>
      {list.data && others.length === 0 ? <p className="muted small mt-2">No other comps in this view to merge into. Widen the rank or time filters to see more.</p> : null}
      {note ? <p className="small mt-2" role="status">{note}</p> : null}
    </section>
  )
}
