/**
 * The app's own building blocks. Interactive ones sit on interior.dev components (see ./interior):
 * press feedback, tooltips, sortable tables, dropdowns, segmented controls and the like button.
 */
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { useMemo, useState, type ComponentPropsWithRef, type ReactNode } from 'react'
import { num, pct, type BoardUnit, type Entity, type Rates, type Scope, type TraitChipData } from './api'
import { Dropdown } from './interior/dropdown'
import { useOptimisticLike } from './interior/like-burst'
import { usePressDepth } from './interior/press-depth'
import { SegmentedControl } from './interior/segmented-control'
import { SortableTable, type SortableColumn } from './interior/sortable-table'
import { Tooltip } from './interior/tooltip-group'
import { BoardArt, TierBadge } from './art'
import { TRAIT_STYLE, type Grade } from './tiers'

// ---- Buttons ---------------------------------------------------------------------
type ButtonProps = Omit<ComponentPropsWithRef<'button'>, 'ref'> & {
  variant?: 'default' | 'primary' | 'ghost'
  size?: 'md' | 'sm'
  square?: boolean
}

/** The one button style. Press feedback comes from interior.dev's press hook. */
export function Button({ variant = 'default', size = 'md', square, className = '', disabled, children, onPointerDown, onKeyDown, onKeyUp, onBlur, ...rest }: ButtonProps) {
  const press = usePressDepth({ disabled })
  return (
    <button
      type="button"
      {...rest}
      ref={press.ref}
      disabled={disabled}
      data-pressed={press.pressed ? '' : undefined}
      className={`btn${variant !== 'default' ? ` ${variant}` : ''}${size === 'sm' ? ' sm' : ''}${square ? ' square' : ''} ${className}`}
      onPointerDown={(e) => { press.bind.onPointerDown(e); onPointerDown?.(e) }}
      onKeyDown={(e) => { press.bind.onKeyDown(e); onKeyDown?.(e) }}
      onKeyUp={(e) => { press.bind.onKeyUp(e); onKeyUp?.(e) }}
      onBlur={(e) => { press.bind.onBlur(); onBlur?.(e) }}
    >
      {children}
    </button>
  )
}

/** A square button with only an icon. The label shows as a tooltip and is read by screen readers. */
export function IconButton({ label, on, onClick, children, side = 'top' }: { label: string; on?: boolean; onClick: () => void; children: ReactNode; side?: 'top' | 'bottom' }) {
  return (
    <Tooltip label={label} side={side}>
      <Button variant="ghost" size="sm" square aria-label={label} aria-pressed={on} className={on ? 'on' : ''}
        onClick={(e) => { e.stopPropagation(); onClick() }}>
        {children}
      </Button>
    </Tooltip>
  )
}

const HEART = 'M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z'
const SPARKS = [0, 60, 120, 180, 240, 300].map((deg) => ({ x: Math.cos((deg * Math.PI) / 180) * 15, y: Math.sin((deg * Math.PI) / 180) * 15 }))

/** Favourite toggle. The optimistic state and the burst come from interior.dev's like hook. */
export function FavButton({ liked: initial, onChange }: { liked: boolean; onChange: (liked: boolean) => void }) {
  const reduced = useReducedMotion()
  const like = useOptimisticLike({ initialLiked: initial, onCommit: async (liked) => onChange(liked) })
  const label = like.liked ? 'Remove from favourites' : 'Add to favourites'
  return (
    <Tooltip label={label}>
      <Button variant="ghost" size="sm" square aria-label={label} aria-pressed={like.liked} className={like.liked ? 'on' : ''}
        onClick={(e) => { e.stopPropagation(); like.toggle() }}>
        <span className="relative grid size-4 place-items-center">
          <svg viewBox="0 0 24 24" className="size-4" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round"><path d={HEART} /></svg>
          <motion.svg viewBox="0 0 24 24" className="absolute inset-0 size-4" fill="currentColor" initial={false}
            animate={{ opacity: like.liked ? 1 : 0, scale: like.liked ? 1 : 0.5 }} transition={reduced ? { duration: 0 } : { type: 'spring', stiffness: 520, damping: 26 }}>
            <path d={HEART} />
          </motion.svg>
          {!reduced && like.burst > 0 && like.liked ? (
            <span key={like.burst} className="pointer-events-none absolute left-1/2 top-1/2 size-0">
              {SPARKS.map((s, i) => (
                <motion.span key={i} className="absolute -ml-[1.5px] -mt-[1.5px] block size-[3px] rounded-full bg-[var(--gold)]"
                  initial={{ x: 0, y: 0, opacity: 0.9 }} animate={{ x: s.x, y: s.y, opacity: 0 }} transition={{ duration: 0.42, ease: 'easeOut' }} />
              ))}
            </span>
          ) : null}
        </span>
      </Button>
    </Tooltip>
  )
}

// ---- Icons and chips ---------------------------------------------------------------
const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' })
/** First two letters or digits of a name, in any script ("아우" for 아우렐리온 솔). */
export const initials = (name: string) =>
  [...graphemes.segment(name)].map((g) => g.segment).filter((g) => /[\p{L}\p{N}]/u.test(g)).slice(0, 2).join('')

/** A unit, item or trait picture. Pass bare when something else (a tooltip) already names it. */
export function Icon({ e, size = 32, plain, bare }: { e: Entity; size?: number; plain?: boolean; bare?: boolean }) {
  const [broken, setBroken] = useState(false)
  const cls = `icon${e.cost && !plain ? ` cost-${e.cost}` : ''}`
  const style = { width: size, height: size, fontSize: Math.max(9, size * 0.3) }
  // No picture (not downloaded yet, or the download failed): initials, or a plain square when too small to read.
  if (!e.icon || broken) return <span className={`${cls} icon-blank`} style={style} title={bare ? undefined : e.name} aria-hidden>{size >= 20 ? initials(e.name) : ''}</span>
  return <img className={cls} style={style} src={e.icon} alt="" title={bare ? undefined : e.name} loading="lazy" onError={() => setBroken(true)} />
}

/** A unit as it sits on a board: portrait, stars above, items below. */
export function UnitChip({ u, size = 40 }: { u: BoardUnit; size?: number }) {
  const tip = u.freq != null ? `${u.name}, on ${pct(u.freq, 0)} of these boards` : u.name
  return (
    <span className="unit-chip" style={{ width: size }}>
      <span className={`stars${u.star && u.star >= 3 ? ' three' : ''}`} aria-label={u.star ? `${u.star} star` : undefined}>{u.star && u.star > 1 ? '★'.repeat(u.star) : ''}</span>
      <Tooltip label={tip}>
        <a href={`#/units/${encodeURIComponent(u.id)}`} aria-label={tip} onClick={(e) => e.stopPropagation()}>
          <Icon e={u} size={size} bare />
        </a>
      </Tooltip>
      <span className="chip-items">
        {(u.items ?? []).map((i, n) => <Icon key={n} e={i} size={Math.floor((size - 4) / 3)} plain />)}
      </span>
    </span>
  )
}

export function TraitChip({ t }: { t: TraitChipData }) {
  return (
    <span className={`trait trait-${TRAIT_STYLE[t.style] ?? 'bronze'}`}>
      <b>{t.units}</b> {t.name}
    </span>
  )
}

export const GradeChip = ({ grade }: { grade: Grade }) => <TierBadge grade={grade} />
export const Tag = ({ kind, children }: { kind: string; children: ReactNode }) => <span className={`tag tag-${kind}`}>{children}</span>

// ---- Numbers ---------------------------------------------------------------------
export const placeTone = (v: number) => (v <= 4.3 ? 'good' : v >= 4.7 ? 'bad' : 'even')

/**
 * Average place as a position on a track, so a column can be read at a glance.
 * The track runs 3.0 to 6.0 (almost nothing averages outside that) with a tick at 4.5, an exactly average result.
 */
export function PlaceRail({ value }: { value: number }) {
  const x = ((Math.min(6, Math.max(3, value)) - 3) / 3) * 100
  const tone = placeTone(value)
  return (
    <span className="rail-cell">
      <span className={`rail-num tone-${tone}`}>{value.toFixed(2)}</span>
      <span className="rail" aria-hidden>
        <span className="rail-mid" />
        <span className={`rail-dot tone-bg-${tone}`} style={{ left: `${x}%` }} />
      </span>
    </span>
  )
}

/** A difference in average place. Negative (better) is green. */
export function Change({ value, suffix }: { value: number | null | undefined; suffix?: string }) {
  if (value == null || Number.isNaN(value)) return <span className="muted">–</span>
  value = Math.round(value * 100) / 100 || 0 // round first, so a tiny negative never shows as "-0.00"
  const tone = value <= -0.05 ? 'good' : value >= 0.05 ? 'bad' : 'even'
  return <span className={`tone-${tone}`}>{value > 0 ? '+' : ''}{value.toFixed(2)}{suffix ? <span className="muted small"> {suffix}</span> : null}</span>
}

export function Fact({ label, children, sub }: { label: string; children: ReactNode; sub?: ReactNode }) {
  return <div className="fact"><dt>{label}</dt><dd>{children}</dd>{sub ? <span className="fact-sub">{sub}</span> : null}</div>
}

/** Horizontal bars for "how often, and how well" breakdowns such as star level. */
export function Dist({ rows }: { rows: { label: string; share: number; avgPlace: number; games: number }[] }) {
  const max = Math.max(...rows.map((r) => r.share), 0.0001)
  return (
    <ul className="dist">
      {rows.map((r) => (
        <li key={r.label} title={`${num(r.games)} games`}>
          <span className="dist-label">{r.label}</span>
          <span className="dist-bar"><span style={{ width: `${(r.share / max) * 100}%` }} /></span>
          <span className="dist-share">{pct(r.share)}</span>
          <span className={`dist-place tone-${placeTone(r.avgPlace)}`}>{r.avgPlace.toFixed(2)}</span>
        </li>
      ))}
    </ul>
  )
}

// ---- Controls --------------------------------------------------------------------
/** interior.dev's segmented control, typed for any string or number value. */
export function Segmented<T extends string | number>({ value, options, onChange, label }: {
  value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label: string
}) {
  return (
    <SegmentedControl className="seg" label={label} value={String(value)}
      options={options.map((o) => ({ value: String(o.value), label: o.label }))}
      onValueChange={(v) => onChange(options.find((o) => String(o.value) === v)!.value)} />
  )
}

/** interior.dev's dropdown as a select. The button shows the current choice, with an optional prefix. */
export function Select<T extends string | number>({ value, options, onChange, prefix, label, quiet }: {
  value: T; options: { value: T; label: string; hint?: string }[]; onChange: (v: T) => void; prefix?: string; label: string
  /** No frame until hovered, for the top bar. */
  quiet?: boolean
}) {
  const current = options.find((o) => o.value === value)
  return (
    <Dropdown className={quiet ? 'dd dd-ghost' : 'dd'} value={String(value)} label={`${prefix ? `${prefix} ` : ''}${current?.label ?? label}`}
      items={options.map((o) => ({ value: String(o.value), label: o.label, hint: o.hint }))}
      onChange={(v) => onChange(options.find((o) => String(o.value) === v)!.value)} />
  )
}

export function Search({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  return <input type="search" className="field search" value={value} placeholder={placeholder} aria-label={placeholder} onChange={(e) => onChange(e.target.value)} />
}

// ---- Tables ----------------------------------------------------------------------
export type Col<T> = {
  key: string; label: string
  /** Give a value to make the column sortable. */
  sort?: (r: T) => number | string
  render: (r: T) => ReactNode
  align?: 'right'
  /** A CSS grid track, e.g. "120px" or "minmax(160px, 2fr)". */
  width?: string
}

/** interior.dev's sortable table: rows glide to their new position when the sort changes. */
export function Table<T extends { id: string }>({ rows, cols, initial, desc = false, empty, label, rowHeight = 48, flush, minWidth = 640 }: {
  rows: T[]; cols: Col<T>[]; initial: string; desc?: boolean; empty?: string; label: string; rowHeight?: number
  /** Narrowest the table may get before it scrolls sideways. Lower it for tables that sit in a half-width column. */
  minWidth?: number
  /** Drop the table's own frame, for when it already sits inside one (a tab panel). */
  flush?: boolean
}) {
  const columns = useMemo<SortableColumn<T>[]>(() => cols.map((c, i) => ({
    id: c.key,
    header: c.label,
    width: c.width ?? (i === 0 ? 'minmax(170px, 2fr)' : 'minmax(0, 1fr)'),
    align: c.align === 'right' ? 'end' : 'start',
    numeric: c.align === 'right',
    sortable: !!c.sort,
    value: c.sort,
    cell: c.render,
  })), [cols])
  if (!rows.length && empty) return <p className="muted px-4 py-6">{empty}</p>
  return (
    <div className="overflow-x-auto">
      <div style={{ minWidth }}>
      <SortableTable rows={rows} columns={columns} getRowId={(r) => r.id} label={label} rowHeight={rowHeight}
        defaultSort={{ columnId: initial, direction: desc ? 'desc' : 'asc' }} className={`${flush ? '!rounded-none !border-0 !bg-transparent !shadow-none' : ''}`} />
      </div>
    </div>
  )
}

export const Games = ({ r }: { r: Rates }) => <>{num(r.games)} <span className="muted small">{pct(r.frequency)}</span></>

export const rateCols = <T extends Rates & { id: string }>(): Col<T>[] => [
  { key: 'place', label: 'Avg place', sort: (r) => r.avgPlace, render: (r) => <PlaceRail value={r.avgPlace} />, width: '150px' },
  { key: 'win', label: 'Win rate', sort: (r) => r.winRate, render: (r) => pct(r.winRate), align: 'right' },
  { key: 'top4', label: 'Top 4', sort: (r) => r.top4Rate, render: (r) => pct(r.top4Rate), align: 'right' },
  { key: 'games', label: 'Games', sort: (r) => r.games, align: 'right', render: (r) => <Games r={r} /> },
]

/** Shown when a page has nothing to list. Says what to do next. */
export function Empty({ scope, what = 'Nothing' }: { scope?: Scope; what?: string }) {
  const none = scope && !scope.patches.length
  return (
    <div className="empty">
      <BoardArt />
      <h2>{none ? 'No stats yet' : `${what} passes these filters`}</h2>
      {none
        ? <p>Collect matches with <code>npm run crawl</code>, then run <code>npm run stats</code>. The Data tab shows progress.</p>
        : <p>Lower the minimum games, pick a wider rank filter, or switch to the whole patch.</p>}
    </div>
  )
}

export function Status({ error, loading }: { error: string | null; loading: boolean }) {
  if (error) return <p className="error">{error}</p>
  return (
    <AnimatePresence>
      {loading ? (
        <motion.div aria-busy className="grid gap-2" initial={{ opacity: 0 }} animate={{ opacity: 1, transition: { delay: 0.15 } }} exit={{ opacity: 0 }}>
          {[92, 100, 84, 96, 78].map((w, i) => <div key={i} className="h-[52px] rounded-[12px] bg-white/[0.035]" style={{ width: `${w}%` }} />)}
        </motion.div>
      ) : null}
    </AnimatePresence>
  )
}
