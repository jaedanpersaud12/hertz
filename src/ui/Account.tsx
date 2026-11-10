import { useState } from 'react'
import { Button, Select } from './parts'

/** Riot's servers, as the client names them. */
export const SERVERS = [
  { value: 'na1', label: 'North America' },
  { value: 'la1', label: 'Latin America North' },
  { value: 'la2', label: 'Latin America South' },
  { value: 'br1', label: 'Brazil' },
  { value: 'euw1', label: 'Europe West' },
  { value: 'eun1', label: 'Europe Nordic & East' },
  { value: 'tr1', label: 'Türkiye' },
  { value: 'ru', label: 'Russia' },
  { value: 'me1', label: 'Middle East' },
  { value: 'kr', label: 'Korea' },
  { value: 'jp1', label: 'Japan' },
  { value: 'oc1', label: 'Oceania' },
  { value: 'sg2', label: 'Southeast Asia' },
  { value: 'tw2', label: 'Taiwan' },
  { value: 'vn2', label: 'Vietnam' },
]

/**
 * Your Riot ID and server, entered in the app. Saved in the database, so .env is not needed for this.
 * Riot is asked straight away, so a typo shows up here rather than later.
 */
export function AccountForm({ riotId = '', platform = 'na1', onSaved, onCancel }: {
  riotId?: string; platform?: string; onSaved: () => void; onCancel?: () => void
}) {
  const [id, setId] = useState(riotId)
  const [server, setServer] = useState(SERVERS.some((s) => s.value === platform) ? platform : 'na1')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const save = async (e?: React.FormEvent) => {
    e?.preventDefault()
    if (!id.includes('#')) return setError('Add your tag after #, for example Name#NA1.')
    setBusy(true)
    setError(null)
    try {
      const r = await fetch('/api/me/account', {
        method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ riotId: id, platform: server }),
      }).then((x) => x.json() as Promise<{ ok: boolean; error?: string | null }>)
      if (r.ok) onSaved()
      else setError(r.error ?? 'Riot did not answer. Try again in a moment.')
    } catch {
      setError('The app could not reach its own server. Is npm run dev still running?')
    } finally {
      setBusy(false)
    }
  }

  return (
    <form className="account-form" onSubmit={save}>
      <label className="account-field">
        <span className="muted small">Riot ID</span>
        <input className="field" value={id} onChange={(e) => setId(e.target.value)} placeholder="Name#TAG" autoComplete="off" spellCheck={false} />
      </label>
      <div className="account-field">
        <span className="muted small">Server</span>
        <Select value={server} options={SERVERS} onChange={setServer} label="Server" />
      </div>
      {error ? <p className="error small">{error}</p> : null}
      <div className="account-actions">
        <Button variant="primary" type="submit" disabled={busy}>{busy ? 'Checking with Riot' : 'Save'}</Button>
        {onCancel ? <Button variant="ghost" onClick={onCancel} disabled={busy}>Cancel</Button> : null}
      </div>
    </form>
  )
}
