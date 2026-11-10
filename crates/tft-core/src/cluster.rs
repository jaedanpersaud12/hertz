//! Decides which "comp" a final board belongs to. Same rule as `src/cluster.ts`:
//! the biggest active trait plus the main carry, e.g. "TFT18_Vanguard|TFT18_Morgana".

use crate::order::locale_cmp;

#[derive(Debug, Clone)]
pub struct KeyUnit<'a> {
    pub character_id: &'a str,
    pub star: i64,
    pub rarity: Option<i64>,
    pub n_items: i64,
}

#[derive(Debug, Clone)]
pub struct KeyTrait<'a> {
    pub name: &'a str,
    pub num_units: i64,
    pub tier_current: i64,
    pub style: i64,
}

/// Riot's "rarity" field on a unit, mapped to its gold cost.
pub fn rarity_cost(rarity: Option<i64>) -> Option<i64> {
    match rarity? {
        0 => Some(1),
        1 => Some(2),
        2 => Some(3),
        4 => Some(4),
        6 => Some(5),
        _ => None,
    }
}

pub fn comp_key(units: &[KeyUnit], traits: &[KeyTrait]) -> Option<String> {
    if units.len() < 4 {
        return None; // knocked out too early to call it a comp
    }

    // Biggest active trait. One-unit traits are unique-champion traits, not a comp's backbone.
    let trait_name = traits
        .iter()
        .filter(|t| t.tier_current > 0 && t.num_units >= 2)
        .min_by(|a, b| {
            b.num_units.cmp(&a.num_units).then(b.style.cmp(&a.style)).then_with(|| locale_cmp(a.name, b.name))
        })
        .map(|t| t.name)
        .unwrap_or("Flex");

    // Main carry: most items, then most expensive, then highest star.
    let carry = units
        .iter()
        .min_by(|a, b| {
            b.n_items
                .cmp(&a.n_items)
                .then(b.rarity.unwrap_or(0).cmp(&a.rarity.unwrap_or(0)))
                .then(b.star.cmp(&a.star))
                .then_with(|| locale_cmp(a.character_id, b.character_id))
        })
        .expect("at least four units");

    Some(format!("{trait_name}|{}", carry.character_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, star: i64, rarity: i64, n: i64) -> KeyUnit<'_> {
        KeyUnit { character_id: id, star, rarity: Some(rarity), n_items: n }
    }
    fn tr(name: &str, num: i64, tier: i64, style: i64) -> KeyTrait<'_> {
        KeyTrait { name, num_units: num, tier_current: tier, style }
    }

    #[test]
    fn biggest_trait_and_most_itemised_unit() {
        let units = [unit("A", 2, 2, 3), unit("B", 2, 4, 0), unit("C", 1, 0, 0), unit("D", 2, 0, 1)];
        let traits = [tr("Big", 6, 3, 4), tr("Small", 2, 1, 1)];
        assert_eq!(comp_key(&units, &traits).as_deref(), Some("Big|A"));
    }

    #[test]
    fn ties_and_edge_cases() {
        // Too few units.
        assert_eq!(comp_key(&[unit("A", 1, 0, 0)], &[]), None);
        // No trait with two or more units: Flex.
        let units = [unit("A", 1, 0, 0), unit("B", 1, 0, 0), unit("C", 1, 0, 0), unit("D", 1, 0, 0)];
        assert_eq!(comp_key(&units, &[tr("Loner", 1, 1, 3)]).as_deref(), Some("Flex|A"));
        // Inactive traits are ignored; equal size breaks on style, then dictionary order.
        let traits = [tr("Zeta", 4, 0, 4), tr("beta", 4, 1, 2), tr("Alpha", 4, 1, 2)];
        assert_eq!(comp_key(&units, &traits).as_deref(), Some("Alpha|A"));
        let traits = [tr("bastion", 4, 1, 2), tr("Bilgewater", 4, 1, 2)];
        assert_eq!(comp_key(&units, &traits).as_deref(), Some("bastion|A"));
        // Same items: more expensive wins, then higher star.
        let units = [unit("A", 3, 0, 3), unit("B", 1, 4, 3), unit("C", 2, 4, 3), unit("D", 1, 0, 0)];
        assert_eq!(comp_key(&units, &[]).as_deref(), Some("Flex|C"));
        // Missing rarity counts as 0.
        let units = [
            KeyUnit { character_id: "X", star: 3, rarity: None, n_items: 2 },
            unit("Y", 1, 0, 2),
            unit("C", 1, 0, 0),
            unit("D", 1, 0, 0),
        ];
        assert_eq!(comp_key(&units, &[]).as_deref(), Some("Flex|X"));
    }

    #[test]
    fn costs() {
        assert_eq!([0, 1, 2, 4, 6].map(|r| rarity_cost(Some(r))), [1, 2, 3, 4, 5].map(Some));
        assert_eq!(rarity_cost(Some(3)), None);
        assert_eq!(rarity_cost(None), None);
    }
}
