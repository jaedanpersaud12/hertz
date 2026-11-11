//! Riot API client. One limiter per host, retries on 429 and server errors, clear errors for key problems.
//!
//! Riot limits each host separately. Matches come from the four routing-group hosts (americas, europe, asia,
//! sea), so each group's host limiter is that group's limiter; server hosts (na1, euw1, ...) count apart.

use crate::limiter::{DEFAULT_LIMITS, Limiter, SlotUse};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tft_core::riot::{Account, LeagueEntry, LeagueList, RankEntry, RiotMatch};

/// What is wrong with the API key. Each case has its own sentence and its own fix, so the crawler and the app
/// (PRD 10's key test) show the same words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProblem {
    /// `RIOT_API_KEY` is empty or unset. No request was sent.
    Missing,
    /// Riot rejected (401 or 403) a key that has worked before.
    Expired,
    /// Riot kept rejecting a key that has never worked, for [`ACTIVATION_WAIT`].
    NotActive,
}

impl KeyProblem {
    /// What is wrong, in one sentence.
    pub fn problem(self) -> &'static str {
        match self {
            KeyProblem::Missing => "Your Riot API key is not set.",
            KeyProblem::Expired => "Your Riot API key has expired. Development keys last 24 hours.",
            KeyProblem::NotActive => "Riot does not accept your API key. It is not active yet, or it is not a valid key.",
        }
    }

    /// What to do about it.
    pub fn fix(self) -> &'static str {
        match self {
            KeyProblem::Missing => {
                "Copy .env.example to .env, put your key from developer.riotgames.com after RIOT_API_KEY=, then start again."
            }
            KeyProblem::Expired => {
                "Generate a new key at developer.riotgames.com, put it after RIOT_API_KEY= in .env, then restart."
            }
            KeyProblem::NotActive => {
                "Check that you copied the whole key into .env. New keys can take a few minutes to start working, \
                 so wait a few minutes after generating it, then try again."
            }
        }
    }
}

impl std::fmt::Display for KeyProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.problem(), self.fix())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RiotError {
    /// The key is missing, expired or not active. The crawler stops all groups on this.
    #[error("{0}")]
    Key(KeyProblem),
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
    /// Riot has accepted this key before: in this process, or in an earlier run (the crawler remembers it).
    key_worked: AtomicBool,
    pub counts: Counts,
}

/// How long a key that has never worked keeps being retried before it is reported (PRD rule 5).
pub const ACTIVATION_WAIT: Duration = Duration::from_secs(120);
/// The spacing of those retries.
const ACTIVATION_RETRY: Duration = Duration::from_secs(15);

/// A fingerprint of the key that can be stored without storing the key: 64-bit FNV-1a, in hex. It cannot be
/// turned back into the key, and it stays the same across builds, so it can be compared on the next start.
pub fn key_fingerprint(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.trim().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}")
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
            key_worked: AtomicBool::new(false),
            counts: Counts::default(),
        }
    }

    /// The fingerprint of this client's key, for remembering that it has worked.
    pub fn key_fingerprint(&self) -> String {
        key_fingerprint(&self.key)
    }

    /// Whether Riot has accepted this key, in this process or (if the caller said so) before.
    pub fn key_worked(&self) -> bool {
        self.key_worked.load(Ordering::Relaxed)
    }

    /// Says the key has worked before (in an earlier run), so a rejection means it has expired and is reported
    /// straight away instead of after the activation wait.
    pub fn set_key_worked(&self) {
        self.key_worked.store(true, Ordering::Relaxed);
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
    /// On 401 or 403 (PRD rule 5): a key that has worked before has expired, and is reported at once. A key
    /// that has never worked may still be activating, so the same request is retried every 15 s, through the
    /// limiter, and [`KeyProblem::NotActive`] is reported only once [`ACTIVATION_WAIT`] has passed. Any success
    /// in between carries on as normal.
    ///
    /// Four intended differences from the TypeScript client:
    /// - 429 retries are uncapped. A 429 waits Retry-After and sends the same request again, and never uses
    ///   up one of the six tries (PRD rule 4). The TypeScript client counts 429s against its tries.
    /// - The request does not check the crawler's stop flag between 429 waits. A stop takes effect once the
    ///   request in hand finishes, which can be several Retry-After waits later; the TypeScript client
    ///   gives up within six attempts.
    /// - The client gives up straight after the sixth failed try. The TypeScript client waits one more
    ///   backoff (32 s) after its last try before it throws.
    /// - Key errors are told apart (missing, expired, not active yet) and a new key gets the 2-minute
    ///   activation wait. The TypeScript client reports every 401 or 403 at once with one sentence. This is a
    ///   deliberate Rust-only improvement (E4); the TypeScript crawler is left as it is.
    pub async fn get<T: DeserializeOwned>(&self, host: &str, path: &str) -> Result<Option<T>, RiotError> {
        if self.key.is_empty() {
            return Err(RiotError::Key(KeyProblem::Missing));
        }
        let url = format!("{}{}", self.base.replace("{host}", host), path);
        let limiter = self.limiter(host);
        let mut failures = 0;
        let mut first_rejected: Option<tokio::time::Instant> = None;

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
            if res.status().is_success() || status == 404 {
                // Riot checks the key before anything else, so a 404 means the key was accepted too.
                self.set_key_worked();
            }
            if res.status().is_success() {
                self.counts.ok.fetch_add(1, Ordering::Relaxed);
                return res.json::<T>().await.map(Some).map_err(|source| RiotError::BadBody { url, source });
            }
            match status {
                404 => return Ok(None),
                401 | 403 => {
                    if self.key_worked() {
                        return Err(RiotError::Key(KeyProblem::Expired));
                    }
                    let since = *first_rejected.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= ACTIVATION_WAIT {
                        return Err(RiotError::Key(KeyProblem::NotActive));
                    }
                    tokio::time::sleep(ACTIVATION_RETRY).await;
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
        client_with_key("test-key", base)
    }

    fn client_with_key(key: &str, base: &str) -> RiotClient {
        init_tls();
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        RiotClient::new(key, base, 1.0).with_http(http)
    }

    fn key_problem(err: RiotError) -> KeyProblem {
        match err {
            RiotError::Key(p) => p,
            other => panic!("expected a key error, got {other:?}"),
        }
    }

    fn gaps(hits: &Hits) -> Vec<Duration> {
        gaps_of(&hits.lock().unwrap())
    }

    fn gaps_of(hits: &[(Instant, String)]) -> Vec<Duration> {
        hits.windows(2).map(|w| w[1].0 - w[0].0).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_honours_retry_after_and_does_not_use_up_a_try() {
        // 8 s is above the client's 5 s fallback, so a client that ignored the header would retry too soon.
        let mut script: Vec<Reply> = (0..7).map(|_| Reply::Http(429, "retry-after: 8\r\n", "")).collect();
        script.push(Reply::Http(200, "", "[\"NA1_1\"]"));
        let (base, hits) = serve(script).await;
        let riot = client(&base);

        let got = riot.get::<Vec<String>>("americas", "/ids").await.unwrap();
        assert_eq!(got, Some(vec!["NA1_1".to_string()]), "seven 429s in a row must not give up the request");
        assert_eq!(hits.lock().unwrap().len(), 8);
        assert!(hits.lock().unwrap().iter().all(|(_, p)| p == "/americas/ids"), "the same request is retried");
        for gap in gaps(&hits) {
            assert!(gap >= Duration::from_secs(8), "retried after {gap:?}, Retry-After said 8 s");
            assert!(gap < Duration::from_millis(9500), "retried after {gap:?}, Retry-After said 8 s");
        }
        assert_eq!(riot.counts.limited.load(Ordering::Relaxed), 7);
        assert_eq!(riot.counts.retried.load(Ordering::Relaxed), 0);
    }

    /// The crawler sends up to 20 fetches at once, so a 429 must pause the whole group, not only the request
    /// that got it (PRD rule 4: block that group). Other regions keep going.
    #[tokio::test(start_paused = true)]
    async fn a_429_blocks_the_whole_group_and_leaves_other_regions_free() {
        // 8 s is above the client's 5 s fallback, so a client that ignored the header would release the group too soon.
        let (base, hits) = serve(vec![Reply::Http(429, "retry-after: 8\r\n", "")]).await;
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
        let a_at = sent("/americas/a");
        assert_eq!(a_at.len(), 2, "A gets its 429, then is sent again");
        let a_gap = a_at[1] - a_at[0];
        assert!(
            a_gap >= Duration::from_secs(8) && a_gap < Duration::from_millis(9500),
            "americas A was sent again {a_gap:?} after its 429; Retry-After said 8 s"
        );
        let b_at = sent("/americas/b");
        assert_eq!(b_at.len(), 1);
        let b_wait = b_at[0] - limited_at;
        assert!(
            b_wait >= Duration::from_secs(8),
            "americas B was sent {b_wait:?} after the 429; Retry-After said 8 s for the whole group"
        );
        assert!(
            b_wait < Duration::from_millis(9500),
            "americas B was sent {b_wait:?} after the 429; Retry-After said 8 s for the whole group"
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

    // ---- Key errors (E4, PRD rule 5) ----------------------------------------------------------------

    #[test]
    fn each_key_problem_has_its_own_sentence_and_fix() {
        let all = [KeyProblem::Missing, KeyProblem::Expired, KeyProblem::NotActive];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.to_string(), format!("{} {}", a.problem(), a.fix()));
            for b in &all[i + 1..] {
                assert_ne!(a.problem(), b.problem());
                assert_ne!(a.fix(), b.fix());
            }
        }
        let missing = KeyProblem::Missing.to_string();
        assert!(missing.contains("not set") && missing.contains(".env") && missing.contains("developer.riotgames.com"));
        let expired = KeyProblem::Expired.to_string();
        assert!(expired.contains("expired") && expired.contains("24 hours"));
        assert!(expired.contains("developer.riotgames.com") && expired.contains(".env") && expired.contains("restart"));
        let not_active = KeyProblem::NotActive.to_string();
        assert!(not_active.contains("not active yet") && not_active.contains("not a valid key"));
        assert!(not_active.contains("whole key") && not_active.contains("wait a few minutes"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_key_is_reported_at_once_and_nothing_is_sent() {
        let (base, hits) = serve(vec![]).await;
        for key in ["", "   "] {
            let riot = client_with_key(key, &base);
            let err = riot.get::<Vec<String>>("americas", "/ids").await.unwrap_err();
            assert_eq!(key_problem(err), KeyProblem::Missing);
        }
        assert_eq!(hits.lock().unwrap().len(), 0, "no request may be sent without a key");
    }

    /// A new key that Riot keeps rejecting is retried for two minutes, then reported as not active.
    #[tokio::test(start_paused = true)]
    async fn a_key_that_has_never_worked_is_retried_for_two_minutes_then_reported_as_not_active() {
        let (base, hits) = serve((0..40).map(|_| Reply::Http(403, "", "{}")).collect()).await;
        let riot = client(&base);
        let started = Instant::now();

        let err = riot.get::<Vec<String>>("americas", "/ids").await.unwrap_err();
        let reported = Instant::now() - started;
        assert_eq!(key_problem(err), KeyProblem::NotActive);
        assert!(reported >= ACTIVATION_WAIT, "reported after {reported:?}, before the 2-minute activation wait");
        let hits = hits.lock().unwrap().clone();
        assert!(hits.iter().all(|(_, p)| p == "/americas/ids"), "the same request is retried");
        let span = hits.last().unwrap().0 - hits[0].0;
        assert!(span >= ACTIVATION_WAIT, "first and last try were {span:?} apart, expected 2 minutes");
        assert!(span < ACTIVATION_WAIT + Duration::from_secs(16), "first and last try were {span:?} apart");
        assert!(hits.len() >= 5 && hits.len() <= 13, "{} tries in two minutes; expected one every 10-15 s", hits.len());
        for gap in gaps_of(&hits) {
            assert!(gap >= Duration::from_secs(10), "retried after {gap:?}");
        }
        assert!(!riot.key_worked());
    }

    /// The key starts working 40 s in: the request carries on as if nothing had happened.
    #[tokio::test(start_paused = true)]
    async fn a_new_key_that_starts_working_within_two_minutes_carries_on_with_no_error() {
        let mut script: Vec<Reply> = (0..3).map(|_| Reply::Http(403, "", "{}")).collect();
        script.push(Reply::Http(200, "", "[\"NA1_9\"]"));
        let (base, hits) = serve(script).await;
        let riot = client(&base);
        let started = Instant::now();

        let got = riot.get::<Vec<String>>("americas", "/ids").await.unwrap();
        assert_eq!(got, Some(vec!["NA1_9".to_string()]));
        assert_eq!(hits.lock().unwrap().len(), 4);
        let took = Instant::now() - started;
        assert!(took >= Duration::from_secs(30) && took < ACTIVATION_WAIT, "took {took:?}");
        assert!(riot.key_worked(), "a success marks the key as working");
    }

    /// A key remembered as working (from an earlier run) that Riot now rejects has expired: no 2-minute wait.
    #[tokio::test(start_paused = true)]
    async fn a_key_that_worked_before_is_reported_expired_at_once() {
        for status in [401, 403] {
            let (base, hits) = serve((0..20).map(|_| Reply::Http(status, "", "{}")).collect()).await;
            let riot = client(&base);
            riot.set_key_worked();
            let started = Instant::now();

            let err = riot.get::<Vec<String>>("americas", "/ids").await.unwrap_err();
            assert_eq!(key_problem(err), KeyProblem::Expired, "status {status}");
            assert_eq!(hits.lock().unwrap().len(), 1, "status {status}: exactly one request");
            assert!(Instant::now() - started < Duration::from_secs(1), "status {status}: reported late");
        }
    }

    /// Within one run, once Riot has accepted the key, a later rejection means it has expired.
    #[tokio::test(start_paused = true)]
    async fn a_rejection_after_a_success_in_the_same_run_is_reported_expired() {
        let (base, hits) =
            serve(vec![Reply::Http(200, "", "[]"), Reply::Http(403, "", "{}"), Reply::Http(403, "", "{}")]).await;
        let riot = client(&base);
        assert!(!riot.key_worked());

        assert_eq!(riot.get::<Vec<String>>("americas", "/ids").await.unwrap(), Some(vec![]));
        assert!(riot.key_worked());
        let started = Instant::now();
        let err = riot.get::<Vec<String>>("europe", "/ids").await.unwrap_err();
        assert_eq!(key_problem(err), KeyProblem::Expired);
        assert_eq!(hits.lock().unwrap().len(), 2);
        assert!(Instant::now() - started < Duration::from_secs(1));
    }

    #[test]
    fn the_key_fingerprint_is_stable_and_does_not_contain_the_key() {
        let key = "RGAPI-00000000-1111-2222-3333-444444444444";
        let fp = key_fingerprint(key);
        assert_eq!(fp, key_fingerprint(&format!(" {key}\n")), "trimmed like the client trims the key");
        assert_ne!(fp, key_fingerprint("RGAPI-00000000-1111-2222-3333-444444444445"));
        assert!(!fp.contains("RGAPI") && !fp.contains("4444"), "{fp}");
        assert_eq!(key_fingerprint("a"), "fnv1a64:af63dc4c8601ec8c", "the same across builds");
    }
}
