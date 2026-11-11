//! Riot API client. One limiter per host, retries on 429 and server errors, clear errors for key problems.
//!
//! Riot limits each host separately. Matches come from the four routing-group hosts (americas, europe, asia,
//! sea), so each group's host limiter is that group's limiter; server hosts (na1, euw1, ...) count apart.

use crate::limiter::{DEFAULT_LIMITS, Limiter, SlotUse};
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

/// Tries per request on server errors and network failures. A 429 does not use one up.
const MAX_TRIES: u32 = 6;

/// Counts one failed try and waits before the next: 1, 2, 4, 8, then 16 seconds.
/// Returns false when the tries are used up.
async fn try_again(failures: &mut u32) -> bool {
    *failures += 1;
    if *failures >= MAX_TRIES {
        return false;
    }
    tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(*failures - 1))).await;
    true
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

    /// How much of a host's long rate-limit window is in use. `None` before the first request to it.
    pub fn slot_use(&self, host: &str) -> Option<SlotUse> {
        self.limiters.lock().unwrap().get(host).map(|l| l.slot_use())
    }

    #[cfg(test)]
    pub(crate) fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
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

    /// GETs a Riot endpoint. `Ok(None)` on 404. Retries on 429 (as often as Riot asks) and on server errors
    /// or network failures (up to [`MAX_TRIES`], then [`RiotError::Unavailable`]). The client does not pause
    /// the host after the last try: the crawler pauses that group for 60 s on `Unavailable`, as the
    /// TypeScript crawler does, and the queued match stays queued.
    ///
    /// Three intended differences from the TypeScript client:
    /// - 429 retries are uncapped. A 429 waits Retry-After and sends the same request again, and never uses
    ///   up one of the six tries (PRD rule 4). The TypeScript client counts 429s against its tries.
    /// - The request does not check the crawler's stop flag between 429 waits. A stop takes effect once the
    ///   request in hand finishes, which can be several Retry-After waits later; the TypeScript client
    ///   gives up within six attempts.
    /// - The client gives up straight after the sixth failed try. The TypeScript client waits one more
    ///   backoff (32 s) after its last try before it throws.
    pub async fn get<T: DeserializeOwned>(&self, host: &str, path: &str) -> Result<Option<T>, RiotError> {
        if self.key.is_empty() {
            return Err(RiotError::Key(
                "RIOT_API_KEY is not set. Copy .env.example to .env and add your key.".into(),
            ));
        }
        let url = format!("{}{}", self.base.replace("{host}", host), path);
        let limiter = self.limiter(host);
        let mut failures = 0;

        loop {
            let permit = limiter.acquire().await;
            let sent = self.http.get(&url).header("X-Riot-Token", &self.key).send().await;
            permit.answered();
            let res = match sent {
                Ok(r) => r,
                Err(_) => {
                    self.counts.retried.fetch_add(1, Ordering::Relaxed);
                    if try_again(&mut failures).await {
                        continue;
                    }
                    break;
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
                    if !try_again(&mut failures).await {
                        break;
                    }
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
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::Instant;

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(super::encode("Jaé Dan#1"), "Ja%C3%A9%20Dan%231");
    }

    /// What the test server does with the next request.
    enum Reply {
        /// Status, extra header lines (each ending in CRLF), body.
        Http(u16, &'static str, &'static str),
        /// Reads the request, then closes the connection without answering.
        HangUp,
        /// Reads the request, then keeps the connection open and never answers (the laptop went to sleep).
        Silent,
    }

    type Hits = Arc<Mutex<Vec<(Instant, String)>>>;

    /// A one-connection-at-a-time HTTP server that plays `script` in order, then answers 200 `[]`.
    /// Returns the client's base URL and the (time, path) of every request it saw.
    async fn serve(script: Vec<Reply>) -> (String, Hits) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits: Hits = Arc::default();
        let seen = Arc::clone(&hits);
        tokio::spawn(async move {
            let mut script = script.into_iter();
            let mut held = Vec::new();
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut req = Vec::new();
                let mut chunk = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&chunk[..n]),
                    }
                }
                let path = String::from_utf8_lossy(&req).split(' ').nth(1).unwrap_or_default().to_string();
                seen.lock().unwrap().push((Instant::now(), path));
                match script.next().unwrap_or(Reply::Http(200, "", "[]")) {
                    Reply::HangUp => drop(sock),
                    Reply::Silent => held.push(sock),
                    Reply::Http(status, headers, body) => {
                        let answer = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                             connection: close\r\n{headers}\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(answer.as_bytes()).await;
                        let _ = sock.shutdown().await;
                    }
                }
            }
        });
        (format!("http://127.0.0.1:{port}/{{host}}"), hits)
    }

    /// No request timeout and no idle pool, so the only timers in a paused-time test are the client's own.
    fn client(base: &str) -> RiotClient {
        init_tls();
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        RiotClient::new("test-key", base, 1.0).with_http(http)
    }

    fn gaps(hits: &Hits) -> Vec<Duration> {
        hits.lock().unwrap().windows(2).map(|w| w[1].0 - w[0].0).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_honours_retry_after_and_does_not_use_up_a_try() {
        let mut script: Vec<Reply> = (0..7).map(|_| Reply::Http(429, "retry-after: 3\r\n", "")).collect();
        script.push(Reply::Http(200, "", "[\"NA1_1\"]"));
        let (base, hits) = serve(script).await;
        let riot = client(&base);

        let got = riot.get::<Vec<String>>("americas", "/ids").await.unwrap();
        assert_eq!(got, Some(vec!["NA1_1".to_string()]), "seven 429s in a row must not give up the request");
        assert_eq!(hits.lock().unwrap().len(), 8);
        assert!(hits.lock().unwrap().iter().all(|(_, p)| p == "/americas/ids"), "the same request is retried");
        for gap in gaps(&hits) {
            assert!(gap >= Duration::from_secs(3), "retried after {gap:?}, Retry-After said 3 s");
        }
        assert_eq!(riot.counts.limited.load(Ordering::Relaxed), 7);
        assert_eq!(riot.counts.retried.load(Ordering::Relaxed), 0);
    }

    /// The crawler sends up to 20 fetches at once, so a 429 must pause the whole group, not only the request
    /// that got it (PRD rule 4: block that group). Other regions keep going.
    #[tokio::test(start_paused = true)]
    async fn a_429_blocks_the_whole_group_and_leaves_other_regions_free() {
        let (base, hits) = serve(vec![Reply::Http(429, "retry-after: 3\r\n", "")]).await;
        let riot = client(&base);

        let a = riot.get::<Vec<String>>("americas", "/a");
        let others = async {
            while riot.counts.limited.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let limited_at = Instant::now();
            let (b, eu) = tokio::join!(
                riot.get::<Vec<String>>("americas", "/b"),
                riot.get::<Vec<String>>("europe", "/eu")
            );
            (limited_at, b.unwrap(), eu.unwrap())
        };
        let (a, (limited_at, b, eu)) = tokio::join!(a, others);
        assert_eq!((a.unwrap(), b, eu), (Some(vec![]), Some(vec![]), Some(vec![])));

        let hits = hits.lock().unwrap().clone();
        let sent = |path: &str| hits.iter().filter(|(_, p)| p == path).map(|(at, _)| *at).collect::<Vec<_>>();
        assert_eq!(sent("/americas/a").len(), 2, "A gets its 429, then is sent again");
        let b_at = sent("/americas/b");
        assert_eq!(b_at.len(), 1);
        assert!(
            b_at[0] - limited_at >= Duration::from_secs(3),
            "americas B was sent {:?} after the 429; Retry-After said 3 s for the whole group",
            b_at[0] - limited_at
        );
        let eu_at = sent("/europe/eu");
        assert_eq!(eu_at.len(), 1);
        assert!(
            eu_at[0] - limited_at < Duration::from_secs(1),
            "europe waited {:?}; an americas 429 must not block it",
            eu_at[0] - limited_at
        );
    }

    /// The client gives up after six tries and leaves the host free: the crawler's own 60 s pause on
    /// `Unavailable` is the only pause, so a batch is not held up twice.
    #[tokio::test(start_paused = true)]
    async fn server_errors_get_six_tries_then_unavailable_and_the_host_is_not_blocked() {
        let script = (0..6).map(|_| Reply::Http(500, "", "")).collect();
        let (base, hits) = serve(script).await;
        let riot = client(&base);

        let err = riot.get::<Vec<String>>("americas", "/ids").await.unwrap_err();
        assert!(matches!(err, RiotError::Unavailable(_)), "got {err:?}");
        assert_eq!(hits.lock().unwrap().len(), 6, "exactly six tries");
        let expected = [1, 2, 4, 8, 16].map(Duration::from_secs);
        let gaps = gaps(&hits);
        assert_eq!(gaps.len(), expected.len());
        for (gap, want) in gaps.into_iter().zip(expected) {
            assert!(gap >= want, "backed off {gap:?}, expected at least {want:?}");
            assert!(gap < want + Duration::from_secs(1), "backed off {gap:?}, expected about {want:?}");
        }
        assert_eq!(riot.counts.retried.load(Ordering::Relaxed), 6);

        // The next request on the same host goes straight away.
        let gave_up = Instant::now();
        assert_eq!(riot.get::<Vec<String>>("americas", "/ids").await.unwrap(), Some(vec![]));
        let (at, path) = hits.lock().unwrap().last().cloned().unwrap();
        assert_eq!(path, "/americas/ids");
        assert!(at - gave_up < Duration::from_secs(1), "americas waited {:?} after giving up", at - gave_up);
    }

    /// A laptop that sleeps mid-crawl leaves a request with no answer. The production client's 20 s timeout
    /// ends it, and the same request goes again (one try used, not the request lost).
    #[tokio::test(start_paused = true)]
    async fn a_request_left_unanswered_by_sleep_times_out_and_is_retried() {
        let (base, hits) = serve(vec![Reply::Silent, Reply::Http(200, "", "[\"NA1_3\"]")]).await;
        init_tls();
        let riot = RiotClient::new("test-key", &base, 1.0); // the production HTTP client, 20 s timeout

        let got = riot.get::<Vec<String>>("americas", "/match").await.unwrap();
        assert_eq!(got, Some(vec!["NA1_3".to_string()]));
        let hits = hits.lock().unwrap().clone();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|(_, p)| p == "/americas/match"), "the same request is retried");
        let gap = hits[1].0 - hits[0].0;
        assert!(gap >= Duration::from_secs(21), "retried after {gap:?}: 20 s timeout plus 1 s backoff");
        assert_eq!(riot.counts.retried.load(Ordering::Relaxed), 1);
        assert_eq!(riot.counts.ok.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn network_failures_are_retried_on_the_same_request() {
        let (base, hits) = serve(vec![Reply::HangUp, Reply::HangUp, Reply::Http(200, "", "[\"NA1_2\"]")]).await;
        let riot = client(&base);

        let got = riot.get::<Vec<String>>("americas", "/match").await.unwrap();
        assert_eq!(got, Some(vec!["NA1_2".to_string()]));
        assert_eq!(hits.lock().unwrap().len(), 3);
        assert!(hits.lock().unwrap().iter().all(|(_, p)| p == "/americas/match"));
        assert_eq!(riot.counts.retried.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn adopts_the_rate_limit_header_and_reports_slot_use() {
        let limited = || Reply::Http(200, "x-app-rate-limit: 1:1,50:120\r\n", "[]");
        let (base, hits) = serve(vec![limited(), limited(), limited()]).await;
        let riot = client(&base);
        assert_eq!(riot.slot_use("americas"), None);

        for _ in 0..3 {
            riot.get::<Vec<String>>("americas", "/ids").await.unwrap();
        }
        for gap in &gaps(&hits) {
            assert!(*gap >= Duration::from_secs(1), "header said 1 per second, gap was {gap:?}");
        }
        let used = riot.slot_use("americas").unwrap();
        assert_eq!((used.last_window, used.riot_limit), (3, 50));
    }
}
