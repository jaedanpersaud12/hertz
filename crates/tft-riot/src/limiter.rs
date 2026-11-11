//! Sliding-window rate limiter for one Riot host. Same rule as the TypeScript `Limiter`.
//!
//! Riot counts limits per host, and the crawler fetches matches only from the four routing-group hosts
//! (americas, europe, asia, sea), so the limiter for a group's host is that group's limiter.
//! Server hosts (na1, euw1, ...) have their own separate limits on Riot's side and their own limiters here.
//!
//! Riot counts a request when it arrives, some time between when we send it and when the answer comes
//! back. So a request holds its slot from the moment it is sent until one full window after its answer
//! arrives. That way no window on Riot's side can ever hold more requests than we allow.
//!
//! Think of a car park with a strict hourly cap: a car counts from the moment it drives in, and its space
//! only frees up a full hour after it leaves.
//!
//! All times are tokio `Instant`s, which are monotonic: changing the wall clock never moves a window.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{Instant, sleep, sleep_until};

#[derive(Debug, Clone, Copy)]
struct Window {
    /// What we allow: Riot's count times the safety margin.
    limit: usize,
    /// What Riot allows, as the header says.
    riot: usize,
    len: Duration,
}

#[derive(Debug)]
struct Slot {
    id: u64,
    sent: Instant,
    /// When the answer arrived. `None` while the request is in flight.
    done: Option<Instant>,
}

#[derive(Debug)]
struct State {
    windows: Vec<Window>,
    slots: Vec<Slot>,
    next_id: u64,
    blocked_until: Option<Instant>,
    /// For [`Limiter::slot_use`]: when the first request went out, and how many have gone out since.
    first_sent: Option<Instant>,
    total_sent: u64,
}

#[derive(Debug)]
pub struct Limiter {
    state: Mutex<State>,
    /// One caller at a time decides whether it may go; the rest queue behind it, in order.
    turn: tokio::sync::Mutex<()>,
    margin: f64,
}

/// Riot's limit for development and personal keys: 20 per second and 100 per two minutes.
pub const DEFAULT_LIMITS: [(u32, u32); 2] = [(20, 1), (100, 120)];

fn to_windows(limits: &[(u32, u32)], margin: f64) -> Vec<Window> {
    limits
        .iter()
        .map(|&(count, secs)| Window {
            limit: ((count as f64 * margin).floor() as usize).max(1),
            riot: count as usize,
            len: Duration::from_secs(secs as u64),
        })
        .collect()
}

impl Limiter {
    pub fn new(limits: &[(u32, u32)], margin: f64) -> Arc<Self> {
        Arc::new(Limiter {
            state: Mutex::new(State {
                windows: to_windows(limits, margin),
                slots: Vec::new(),
                next_id: 0,
                blocked_until: None,
                first_sent: None,
                total_sent: 0,
            }),
            turn: tokio::sync::Mutex::new(()),
            margin,
        })
    }

    /// Riot reports the real limits in a header like "20:1,100:120". Adopt them, so a bigger key just works.
    pub fn update_from_header(&self, header: Option<&str>) {
        let Some(h) = header else { return };
        let parsed: Vec<(u32, u32)> = h
            .split(',')
            .filter_map(|p| {
                let (a, b) = p.trim().split_once(':')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            })
            .collect();
        if !parsed.is_empty() {
            self.state.lock().unwrap().windows = to_windows(&parsed, self.margin);
        }
    }

    /// Stops all requests on this host for a while (used after a 429).
    pub fn block(&self, d: Duration) {
        let until = Instant::now() + d;
        let mut st = self.state.lock().unwrap();
        st.blocked_until = Some(st.blocked_until.map_or(until, |b| b.max(until)));
    }

    /// Waits until it is safe to send one request. Call [`Permit::answered`] when the answer arrives.
    pub async fn acquire(self: &Arc<Self>) -> Permit {
        let _turn = self.turn.lock().await;
        loop {
            let now = Instant::now();
            let wait = {
                let mut st = self.state.lock().unwrap();
                match st.blocked_until {
                    Some(b) if b > now => Err(b),
                    _ => {
                        let longest = st.windows.iter().map(|w| w.len).max().unwrap_or_default();
                        st.slots.retain(|s| s.done.is_none_or(|d| d + longest > now));
                        let mut wait = Duration::ZERO;
                        for w in &st.windows {
                            let held: Vec<Option<Instant>> =
                                st.slots.iter().filter(|s| s.done.is_none_or(|d| d + w.len > now)).map(|s| s.done).collect();
                            if held.len() < w.limit {
                                continue;
                            }
                            // This many slots must free up before one more request fits.
                            let need = held.len() - w.limit + 1;
                            let mut finished: Vec<Instant> = held.into_iter().flatten().collect();
                            finished.sort();
                            let w_wait = if finished.len() >= need {
                                (finished[need - 1] + w.len + Duration::from_millis(5)).saturating_duration_since(now)
                            } else {
                                Duration::from_millis(50) // wait for answers still in flight
                            };
                            wait = wait.max(w_wait);
                        }
                        if wait.is_zero() {
                            let id = st.next_id;
                            st.next_id += 1;
                            st.slots.push(Slot { id, sent: now, done: None });
                            st.first_sent.get_or_insert(now);
                            st.total_sent += 1;
                            return Permit { limiter: Arc::clone(self), id, open: true };
                        }
                        Ok(wait)
                    }
                }
            };
            match wait {
                Err(until) => sleep_until(until).await,
                Ok(d) => sleep(d).await,
            }
        }
    }

    /// How many of Riot's long-window slots this host uses (e.g. 95 of 100 per 2 minutes).
    pub fn slot_use(&self) -> SlotUse {
        let now = Instant::now();
        let st = self.state.lock().unwrap();
        let Some(w) = st.windows.iter().max_by_key(|w| w.len) else {
            return SlotUse::default();
        };
        // Windows elapsed since the first request, at least one so a fresh start does not read as huge.
        let windows = st.first_sent.map_or(1.0, |f| (now - f).as_secs_f64() / w.len.as_secs_f64()).max(1.0);
        SlotUse {
            last_window: st.slots.iter().filter(|s| s.sent + w.len > now).count(),
            average: st.total_sent as f64 / windows,
            riot_limit: w.riot,
            window: w.len,
        }
    }

    fn mark_done(&self, id: u64) {
        let mut st = self.state.lock().unwrap();
        if let Some(s) = st.slots.iter_mut().find(|s| s.id == id) {
            s.done = Some(Instant::now());
        }
    }
}

/// How full one host's long window is.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SlotUse {
    /// Requests sent in the last long window.
    pub last_window: usize,
    /// Requests sent per long window, averaged since this host's first request. A single window swings
    /// with where it starts (requests go out in bursts of 20), so this is the number to judge a long run by.
    pub average: f64,
    /// Riot's limit for the long window, before the safety margin.
    pub riot_limit: usize,
    pub window: Duration,
}

/// One request's place in the window. Marked answered when dropped, so an early return never leaks a slot.
#[derive(Debug)]
pub struct Permit {
    limiter: Arc<Limiter>,
    id: u64,
    open: bool,
}

impl Permit {
    pub fn answered(mut self) {
        self.close();
    }
    fn close(&mut self) {
        if std::mem::take(&mut self.open) {
            self.limiter.mark_done(self.id);
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulated time: a minute of traffic in milliseconds of real time.
    #[tokio::test(start_paused = true)]
    async fn never_exceeds_any_window_and_stays_close_to_the_limit() {
        let limiter = Limiter::new(&[(10, 1), (50, 10)], 1.0);
        let sent = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let start = Instant::now();
        let mut tasks = Vec::new();
        for i in 0..20u64 {
            let (l, sent) = (Arc::clone(&limiter), Arc::clone(&sent));
            tasks.push(tokio::spawn(async move {
                while Instant::now() < start + Duration::from_secs(60) {
                    let permit = l.acquire().await;
                    sent.lock().unwrap().push(Instant::now());
                    // Latency between 20 and 400 ms, different per caller.
                    sleep(Duration::from_millis(20 + (i * 37) % 380)).await;
                    permit.answered();
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let sent = sent.lock().unwrap();
        for (limit, secs) in [(10, 1), (50, 10)] {
            let w = Duration::from_secs(secs);
            for (i, t) in sent.iter().enumerate() {
                let n = sent[i..].iter().take_while(|u| **u < *t + w).count();
                assert!(n <= limit, "{n} requests in a {secs}s window, limit {limit}");
            }
        }
        // 60 s at 50 per 10 s is at most 300 + 50. Holding slots a window past the answer costs some of that.
        assert!(sent.len() >= 250, "only {} requests in 60 s", sent.len());
    }

    #[tokio::test(start_paused = true)]
    async fn adopts_header_limits_and_honours_blocks() {
        let limiter = Limiter::new(&DEFAULT_LIMITS, 1.0);
        limiter.update_from_header(Some("2:1, 100:120"));
        let t0 = Instant::now();
        for _ in 0..3 {
            limiter.acquire().await.answered();
        }
        assert!(Instant::now() - t0 >= Duration::from_secs(1), "third request must wait for the 1 s window");

        limiter.block(Duration::from_secs(5));
        let t1 = Instant::now();
        drop(limiter.acquire().await);
        assert!(Instant::now() - t1 >= Duration::from_secs(5));
        limiter.update_from_header(Some("garbage"));
        assert_eq!(limiter.state.lock().unwrap().windows[0].limit, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slot_frees_one_window_after_the_answer_not_after_sending() {
        let limiter = Limiter::new(&[(1, 1)], 1.0);
        let t0 = Instant::now();
        let permit = limiter.acquire().await;
        sleep(Duration::from_secs(3)).await; // a slow answer
        permit.answered();
        drop(limiter.acquire().await);
        let waited = Instant::now() - t0;
        assert!(waited >= Duration::from_secs(4), "second request went out {waited:?} after the first");
        assert!(waited < Duration::from_millis(4100), "second request waited too long: {waited:?}");
    }

    /// Callers that always have work, with 50 to 400 ms answers, for 24 simulated hours (the soak's length).
    async fn sustained_load(margin: f64) -> (Vec<Instant>, SlotUse) {
        let limiter = Limiter::new(&DEFAULT_LIMITS, margin);
        let sent = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let end = Instant::now() + Duration::from_secs(24 * 3600);
        let mut tasks = Vec::new();
        for i in 0..30u64 {
            let (l, sent) = (Arc::clone(&limiter), Arc::clone(&sent));
            tasks.push(tokio::spawn(async move {
                while Instant::now() < end {
                    let permit = l.acquire().await;
                    sent.lock().unwrap().push(Instant::now());
                    sleep(Duration::from_millis(50 + (i * 53) % 350)).await;
                    permit.answered();
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let use_at_end = limiter.slot_use();
        let mut sent = Arc::try_unwrap(sent).unwrap().into_inner().unwrap();
        sent.sort();
        (sent, use_at_end)
    }

    /// Most requests in any window of `len` (every window that starts at a send).
    fn most_in_any(sent: &[Instant], len: Duration) -> usize {
        (0..sent.len()).map(|i| sent[i..].iter().take_while(|u| **u < sent[i] + len).count()).max().unwrap()
    }

    /// Requests per 2 minutes in steady state: the first 2-minute window (warm-up, when nothing is held yet)
    /// is skipped, then every whole 2-minute window up to the last request is counted.
    fn steady_per_two_minutes(sent: &[Instant]) -> f64 {
        let w = Duration::from_secs(120);
        let from = sent[0] + w;
        let windows = ((*sent.last().unwrap() - from).as_secs_f64() / w.as_secs_f64()).floor();
        let to = from + w.mul_f64(windows);
        sent.iter().filter(|t| **t >= from && **t < to).count() as f64 / windows
    }

    /// At the default RATE_MARGIN, a region uses at least 95 of Riot's 100 slots per 2 minutes, all day,
    /// and never more than 100 per 2 minutes or 20 per second.
    #[tokio::test(start_paused = true)]
    async fn a_day_at_the_default_margin_uses_at_least_95_of_100_slots_and_never_more_than_riot_allows() {
        let margin = tft_core::config::Config::from_lookup(|_| None).rate_margin;
        let (sent, at_end) = sustained_load(margin).await;
        let most = most_in_any(&sent, Duration::from_secs(120));
        assert!(most <= 100, "{most} requests in one 2-minute window, Riot allows 100");
        let most = most_in_any(&sent, Duration::from_secs(1));
        assert!(most <= 20, "{most} requests in one second, Riot allows 20");
        let used = steady_per_two_minutes(&sent);
        eprintln!("steady state at RATE_MARGIN={margin}: {used:.2} of 100 slots per 2 minutes over 24 hours");
        assert!(used >= 95.0, "only {used:.2} of 100 slots used per 2 minutes");
        assert_eq!(at_end.riot_limit, 100);
        assert!((at_end.average - used).abs() < 2.0, "slot use reports {}, actual {used:.2}", at_end.average);
    }

    #[tokio::test(start_paused = true)]
    async fn slot_use_counts_requests_per_two_minutes() {
        let limiter = Limiter::new(&DEFAULT_LIMITS, 1.0);
        let empty = SlotUse { last_window: 0, average: 0.0, riot_limit: 100, window: Duration::from_secs(120) };
        assert_eq!(limiter.slot_use(), empty);
        let t0 = Instant::now();
        for _ in 0..30 {
            limiter.acquire().await.answered();
        }
        let u = limiter.slot_use();
        assert_eq!((u.last_window, u.average), (30, 30.0));
        // Four minutes after the first request: 30 requests over two windows.
        tokio::time::advance(Duration::from_secs(240) - (Instant::now() - t0)).await;
        let u = limiter.slot_use();
        assert_eq!((u.last_window, u.average), (0, 15.0));
        limiter.update_from_header(Some("500:10,30000:600"));
        assert_eq!(limiter.slot_use().riot_limit, 30000);
    }

    /// Windows run on monotonic time. When the clock jumps forward (a laptop waking from sleep on Windows,
    /// where monotonic time includes sleep), every window has passed and requests go out at once.
    #[tokio::test(start_paused = true)]
    async fn windows_are_free_again_after_a_long_sleep() {
        let limiter = Limiter::new(&DEFAULT_LIMITS, 1.0);
        for _ in 0..100 {
            limiter.acquire().await.answered();
        }
        tokio::time::advance(Duration::from_secs(10 * 60)).await;
        let t0 = Instant::now();
        for _ in 0..20 {
            limiter.acquire().await.answered();
        }
        assert_eq!(Instant::now(), t0, "requests after a sleep must not wait");
    }
}
