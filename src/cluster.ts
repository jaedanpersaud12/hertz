/**
 * Decides which "comp" a final board belongs to.
 *
 * A comp is identified by its biggest trait plus its main carry, for example "TFT18_Vanguard|TFT18_Morgana".
 * That is deliberately simple: it is stable from patch to patch, cheap to compute while storing a match,
 * and it lines up with how players name comps ("Vanguard Morgana").
 *
 * Change the rules here, then run "npm run recluster" to re-label every stored board.
 */
export type KeyUnit = { character_id: string; star: number; rarity: number | null; n_items: number }
export type KeyTrait = { name: string; num_units: number; tier_current: number; style: number }

/** Riot's "rarity" field on a unit, mapped to its gold cost. */
export const rarityCost = (rarity: number | null | undefined): number | null =>
  rarity == null ? null : ({ 0: 1, 1: 2, 2: 3, 4: 4, 6: 5 } as Record<number, number>)[rarity] ?? null

export function compKey(units: KeyUnit[], traits: KeyTrait[]): string | null {
  if (units.length < 4) return null // knocked out too early to call it a comp

  // Biggest active trait. One-unit traits are unique-champion traits, not a comp's backbone.
  const trait = traits
    .filter((t) => t.tier_current > 0 && t.num_units >= 2)
    .sort((a, b) => b.num_units - a.num_units || b.style - a.style || a.name.localeCompare(b.name))[0]

  // Main carry: most items, then most expensive, then highest star.
  const carry = [...units].sort(
    (a, b) => b.n_items - a.n_items || (b.rarity ?? 0) - (a.rarity ?? 0) || b.star - a.star || a.character_id.localeCompare(b.character_id),
  )[0]

  return `${trait?.name ?? 'Flex'}|${carry.character_id}`
}
