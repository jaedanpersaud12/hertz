//! Sliding-window rate limiter for one Riot host. Same rule as the TypeScript `Limiter`.
//!
//! Riot counts a request when it arrives, some time between when we send it and when the answer comes
//! back. So a request holds its slot from the moment it is sent until one full window after its answer
//! arrives. That way no window on Riot's side can ever hold more requests than we allow.
//!
//! Think of a car park with a strict hourly cap: a car counts from the moment it drives in, and its space
//! only frees up a full hour after it leaves.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{Instant, sleep, sleep_until};

#[derive(Debug, Clone, Copy)]
struct Window {
    limit: usize,
    len: Duration,
}

#[derive(Debug)]
struct Slot {
    id: u64,
    /// When the answer arrived. `None` while the request is in flight.
    done: Option<Instant>,
}

#[derive(Debug)]
struct State {
    windows: Vec<Window>,
    slots: Vec<Slot>,
    next_id: u64,
    blocked_until: Option<Instant>,
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
                            st.slots.push(Slot { id, done: None });
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

    fn mark_done(&self, id: u64) {
        let mut st = self.state.lock().unwrap();
        if let Some(s) = st.slots.iter_mut().find(|s| s.id == id) {
            s.done = Some(Instant::now());
        }
    }
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
}
