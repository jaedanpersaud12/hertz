//! The shapes of Riot's TFT responses that the engine reads. Unknown fields are ignored and missing
//! ones fall back to defaults, so a new field or a dropped one never stops the crawler.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RiotMatch {
    pub metadata: Metadata,
    pub info: Info,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub match_id: String,
    #[serde(default)]
    pub participants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    pub game_datetime: i64,
    #[serde(default)]
    pub game_length: Option<f64>,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub queue_id: Option<i64>,
    #[serde(default, rename = "queueId")]
    pub queue_id_camel: Option<i64>,
    #[serde(default)]
    pub tft_set_number: Option<i64>,
    #[serde(default)]
    pub participants: Vec<Participant>,
}

impl Info {
    /// Riot has used both spellings.
    pub fn queue(&self) -> Option<i64> {
        self.queue_id.or(self.queue_id_camel)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Participant {
    pub puuid: String,
    #[serde(default, rename = "riotIdGameName")]
    pub riot_id_game_name: Option<String>,
    #[serde(default, rename = "riotIdTagline")]
    pub riot_id_tagline: Option<String>,
    pub placement: i64,
    #[serde(default)]
    pub level: Option<i64>,
    #[serde(default)]
    pub last_round: Option<i64>,
    #[serde(default)]
    pub gold_left: Option<i64>,
    #[serde(default)]
    pub total_damage_to_players: Option<i64>,
    #[serde(default)]
    pub traits: Vec<Trait>,
    #[serde(default)]
    pub units: Vec<Unit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trait {
    pub name: String,
    pub num_units: i64,
    #[serde(default)]
    pub style: i64,
    pub tier_current: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Unit {
    pub character_id: String,
    /// Star level.
    pub tier: i64,
    #[serde(default)]
    pub rarity: Option<i64>,
    #[serde(default, rename = "itemNames")]
    pub item_names: Option<Vec<String>>,
}

impl Unit {
    /// Item ids with blanks removed, in Riot's order.
    pub fn items(&self) -> Vec<String> {
        self.item_names.iter().flatten().filter(|i| !i.is_empty()).cloned().collect()
    }
}

/// One entry of a league list. Riot added `puuid`; older responses only had summoner ids.
#[derive(Debug, Clone, Deserialize)]
pub struct LeagueEntry {
    #[serde(default)]
    pub puuid: Option<String>,
    #[serde(default, rename = "leaguePoints")]
    pub league_points: Option<i64>,
    #[serde(default)]
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeagueList {
    #[serde(default)]
    pub entries: Vec<LeagueEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub puuid: String,
    #[serde(rename = "gameName")]
    pub game_name: String,
    #[serde(rename = "tagLine")]
    pub tag_line: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RankEntry {
    #[serde(rename = "queueType")]
    pub queue_type: String,
    pub tier: String,
    pub rank: String,
    #[serde(rename = "leaguePoints")]
    pub league_points: i64,
    pub wins: i64,
    pub losses: i64,
}
