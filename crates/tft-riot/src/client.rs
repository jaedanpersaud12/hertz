//! Riot API client. One limiter per host, retries on 429 and server errors, clear errors for key problems.

use crate::limiter::{DEFAULT_LIMITS, Limiter};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tft_core::riot::{Account, LeagueEntry, LeagueList, RankEntry, RiotMatch};

#[derive(Debug, thiserror::Error)]
pub enum RiotError {
    /// The key is missing, expired or not allowed. The crawler stops on this.
    #[error("{0}")]
    Key(String),
    /// Riot kept failing or could not be reached. Nothing is lost: the caller waits and tries again.
    #[error("{0}")]
    Unavailable(String),
    /// Riot answered 200 with something we could not read. Retrying would give the same answer.
    #[error("unreadable answer from {url}: {source}")]
    BadBody { url: String, source: reqwest::Error },
}

#[derive(Debug, Default)]
pub struct Counts {
    pub ok: AtomicU64,
    pub retried: AtomicU64,
    pub limited: AtomicU64,
}

pub struct RiotClient {
    http: reqwest::Client,
    key: String,
    base: String,
    margin: f64,
    limiters: Mutex<HashMap<String, Arc<Limiter>>>,
    pub counts: Counts,
}

/// Installs the TLS backend once per process. Safe to call more than once.
pub fn init_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A plain HTTP client for non-Riot downloads (Community Dragon).
pub fn http_client() -> reqwest::Client {
    init_tls();
    reqwest::Client::builder()
        .user_agent(concat!("tft-local/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .expect("HTTP client")
}

impl RiotClient {
    pub fn new(key: &str, base: &str, margin: f64) -> Self {
        RiotClient {
            http: http_client(),
            key: key.trim().to_string(),
            base: base.to_string(),
            margin,
            limiters: Mutex::new(HashMap::new()),
            counts: Counts::default(),
        }
    }

    fn limiter(&self, host: &str) -> Arc<Limiter> {
        Arc::clone(
            self.limiters
                .lock()
                .unwrap()
                .entry(host.to_string())
                .or_insert_with(|| Limiter::new(&DEFAULT_LIMITS, self.margin)),
        )
    }

    /// GETs a Riot endpoint. `Ok(None)` on 404. Retries on 429 and server errors.
    pub async fn get<T: DeserializeOwned>(&self, host: &str, path: &str) -> Result<Option<T>, RiotError> {
        if self.key.is_empty() {
            return Err(RiotError::Key(
                "RIOT_API_KEY is not set. Copy .env.example to .env and add your key.".into(),
            ));
        }
        let url = format!("{}{}", self.base.replace("{host}", host), path);
        let limiter = self.limiter(host);
        let backoff = |attempt: u32| tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(attempt)));

        for attempt in 0..6 {
            let permit = limiter.acquire().await;
            let sent = self.http.get(&url).header("X-Riot-Token", &self.key).send().await;
            permit.answered();
            let res = match sent {
                Ok(r) => r,
                Err(_) => {
                    self.counts.retried.fetch_add(1, Ordering::Relaxed);
                    backoff(attempt).await;
                    continue;
                }
            };
            limiter.update_from_header(res.headers().get("x-app-rate-limit").and_then(|v| v.to_str().ok()));

            let status = res.status().as_u16();
            if res.status().is_success() {
                self.counts.ok.fetch_add(1, Ordering::Relaxed);
                return res.json::<T>().await.map(Some).map_err(|source| RiotError::BadBody { url, source });
            }
            match status {
                404 => return Ok(None),
                401 | 403 => {
                    return Err(RiotError::Key(format!(
                        "Riot rejected the API key ({status}) on {host}. Development keys expire every 24 hours: \
                         generate a new one at developer.riotgames.com and update .env."
                    )));
                }
                429 => {
                    self.counts.limited.fetch_add(1, Ordering::Relaxed);
                    let retry_after = res
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<f64>().ok())
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .unwrap_or(5.0);
                    limiter.block(Duration::from_millis((retry_after * 1000.0) as u64 + 250));
                }
                _ => {
                    self.counts.retried.fetch_add(1, Ordering::Relaxed);
                    backoff(attempt).await;
                }
            }
        }
        Err(RiotError::Unavailable(format!(
            "Riot did not answer on {host} after several tries. Check your connection, or try again in a few minutes."
        )))
    }

    // ---- Endpoints ---------------------------------------------------------------------------

    /// Challenger, Grandmaster and Master come back as one list each.
    pub async fn apex_league(&self, platform: &str, tier: &str) -> Result<Vec<LeagueEntry>, RiotError> {
        let path = format!("/tft/league/v1/{}?queue=RANKED_TFT", tier.to_lowercase());
        Ok(self.get::<LeagueList>(platform, &path).await?.map(|l| l.entries).unwrap_or_default())
    }

    /// Diamond and below are paged per division. An empty page means the division is exhausted.
    pub async fn league_page(
        &self,
        platform: &str,
        tier: &str,
        division: &str,
        page: u32,
    ) -> Result<Vec<LeagueEntry>, RiotError> {
        let path = format!("/tft/league/v1/entries/{tier}/{division}?queue=RANKED_TFT&page={page}");
        Ok(self.get::<Vec<LeagueEntry>>(platform, &path).await?.unwrap_or_default())
    }

    pub async fn match_ids(
        &self,
        route: &str,
        puuid: &str,
        start_time_sec: Option<i64>,
        count: u32,
    ) -> Result<Vec<String>, RiotError> {
        let since = start_time_sec.filter(|s| *s != 0).map(|s| format!("&startTime={s}")).unwrap_or_default();
        let path = format!("/tft/match/v1/matches/by-puuid/{puuid}/ids?start=0&count={count}{since}");
        Ok(self.get::<Vec<String>>(route, &path).await?.unwrap_or_default())
    }

    pub async fn get_match(&self, route: &str, match_id: &str) -> Result<Option<RiotMatch>, RiotError> {
        self.get(route, &format!("/tft/match/v1/matches/{match_id}")).await
    }

    pub async fn account(&self, route: &str, name: &str, tag: &str) -> Result<Option<Account>, RiotError> {
        let path = format!("/riot/account/v1/accounts/by-riot-id/{}/{}", encode(name), encode(tag));
        self.get(route, &path).await
    }

    pub async fn rank(&self, platform: &str, puuid: &str) -> Result<Vec<RankEntry>, RiotError> {
        Ok(self.get(platform, &format!("/tft/league/v1/by-puuid/{puuid}")).await?.unwrap_or_default())
    }
}

/// `encodeURIComponent` for Riot IDs.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(super::encode("Jaé Dan#1"), "Ja%C3%A9%20Dan%231");
    }
}
