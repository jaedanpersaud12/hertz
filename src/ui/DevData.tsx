import { STRESS } from './api'

const MODES = [['', 'Demo data'], ['worst', 'Worst case'], ['empty', 'Empty'], ['one', 'One'], ['many', '1,000 rows']] as const

/**
 * Dev-only switch for stress-testing the interface. It is test equipment, not part of the design,
 * so it is deliberately plain. The choice lives in the URL (?data=worst) so a reload keeps it.
 */
export function DevData() {
  const pick = (mode: string) => {
    const url = new URL(location.href)
    if (mode) url.searchParams.set('data', mode); else url.searchParams.delete('data')
    location.href = url.toString()
  }
  return (
    <div style={{ position: 'fixed', bottom: 10, left: '50%', transform: 'translateX(-50%)', zIndex: 100, display: 'flex', gap: 2, padding: 3,
      background: '#d4d4d8', borderRadius: 8, font: '12px system-ui, sans-serif', boxShadow: '0 2px 10px rgba(0,0,0,.5)' }}>
      {MODES.map(([mode, label]) => (
        <button key={mode} onClick={() => pick(mode)} aria-pressed={(STRESS ?? '') === mode}
          style={{ padding: '4px 9px', borderRadius: 6, color: '#18181b', background: (STRESS ?? '') === mode ? '#fff' : 'transparent', whiteSpace: 'nowrap' }}>
          {label}
        </button>
      ))}
    </div>
  )
}
