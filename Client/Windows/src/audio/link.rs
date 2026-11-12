//! Voice connection stats: encrypted UDP pings, packet counters and a
//! bounded ping history for the connection panel. RAM only.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusttalk_shared::secure::ReplayWindow;

pub const PING_INTERVAL: Duration = Duration::from_secs(2);
/// How far back the panel looks.
pub const HISTORY: Duration = Duration::from_secs(240);
/// Upper bound regardless of timing (240 s / 2 s = 120 samples).
pub const MAX_SAMPLES: usize = 128;
/// Unanswered pings kept for matching pongs.
const MAX_PENDING: usize = 8;
const PENDING_TIMEOUT: Duration = Duration::from_secs(10);

/// Counters at the time of one answered ping. All wrap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Audio packets we had sent when the ping left.
    pub sent: u32,
    /// Audio packets the server had accepted from us.
    pub server_received: u32,
    /// Audio packets the server had relayed to us.
    pub server_relayed: u32,
    /// Audio packets we had received when the pong arrived.
    pub received: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub at: Instant,
    pub rtt_ms: f32,
    pub counters: Counters,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Summary {
    pub last_ms: Option<f32>,
    pub average_ms: Option<f32>,
    /// Mean change between consecutive pings.
    pub jitter_ms: Option<f32>,
    /// Percent; `None` while no audio flowed in that direction.
    pub outbound_loss: Option<f32>,
    pub inbound_loss: Option<f32>,
}

/// Loss in percent between two counter snapshots. A backwards or wrapped
/// jump larger than half the range counts as "no data".
pub fn loss_percent(expected: u32, got: u32) -> Option<f32> {
    if expected == 0 || expected > u32::MAX / 2 || got > u32::MAX / 2 {
        return None;
    }
    Some(expected.saturating_sub(got) as f32 * 100.0 / expected as f32)
}

/// Bounded ping history.
#[derive(Debug, Default)]
pub struct History {
    samples: VecDeque<Sample>,
}

impl History {
    pub fn push(&mut self, s: Sample) {
        self.samples.push_back(s);
        while self.samples.len() > MAX_SAMPLES
            || self
                .samples
                .front()
                .is_some_and(|f| s.at.saturating_duration_since(f.at) > HISTORY)
        {
            self.samples.pop_front();
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// (seconds ago, ping ms), oldest first, only within [`HISTORY`].
    pub fn points(&self, now: Instant) -> Vec<(f32, f32)> {
        self.samples
            .iter()
            .map(|s| (now.saturating_duration_since(s.at).as_secs_f32(), s.rtt_ms))
            .filter(|(age, _)| *age <= HISTORY.as_secs_f32())
            .collect()
    }

    pub fn summary(&self) -> Summary {
        let (Some(first), Some(last)) = (self.samples.front(), self.samples.back()) else {
            return Summary::default();
        };
        let n = self.samples.len() as f32;
        let average = self.samples.iter().map(|s| s.rtt_ms).sum::<f32>() / n;
        let jitter = (self.samples.len() > 1).then(|| {
            let diffs: f32 = self
                .samples
                .iter()
                .zip(self.samples.iter().skip(1))
                .map(|(a, b)| (b.rtt_ms - a.rtt_ms).abs())
                .sum();
            diffs / (n - 1.0)
        });
        let (a, b) = (first.counters, last.counters);
        Summary {
            last_ms: Some(last.rtt_ms),
            average_ms: Some(average),
            jitter_ms: jitter,
            outbound_loss: loss_percent(
                b.sent.wrapping_sub(a.sent),
                b.server_received.wrapping_sub(a.server_received),
            ),
            inbound_loss: loss_percent(
                b.server_relayed.wrapping_sub(a.server_relayed),
                b.received.wrapping_sub(a.received),
            ),
        }
    }
}

/// Shared between the audio threads and the UI (lives in `AudioControl`).
#[derive(Debug, Default)]
pub struct LinkStats {
    /// Audio packets sent (voice + screen audio).
    pub audio_sent: AtomicU32,
    /// Authenticated audio packets received.
    pub audio_received: AtomicU32,
    next_ping_id: AtomicU32,
    /// Replay window for packets from the server; survives engine restarts.
    pub replay: Mutex<ReplayWindow>,
    history: Mutex<History>,
}

impl LinkStats {
    /// New session (server counters start at 0 again).
    pub fn reset(&self) {
        self.audio_sent.store(0, Ordering::Relaxed);
        self.audio_received.store(0, Ordering::Relaxed);
        *self.replay.lock().unwrap() = ReplayWindow::default();
        self.history.lock().unwrap().clear();
    }

    pub fn next_ping_id(&self) -> u32 {
        self.next_ping_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn record(&self, s: Sample) {
        self.history.lock().unwrap().push(s);
    }

    pub fn view(&self, now: Instant) -> (Vec<(f32, f32)>, Summary) {
        let h = self.history.lock().unwrap();
        (h.points(now), h.summary())
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    id: u32,
    at: Instant,
    sent: u32,
}

/// Ping bookkeeping of the receive thread.
#[derive(Debug, Default)]
pub struct Pinger {
    pending: VecDeque<Pending>,
    last_sent: Option<Instant>,
}

impl Pinger {
    pub fn due(&self, now: Instant) -> bool {
        self.last_sent
            .is_none_or(|t| now.saturating_duration_since(t) >= PING_INTERVAL)
    }

    pub fn sent(&mut self, id: u32, now: Instant, audio_sent: u32) {
        self.last_sent = Some(now);
        self.pending
            .retain(|p| now.saturating_duration_since(p.at) < PENDING_TIMEOUT);
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(Pending {
            id,
            at: now,
            sent: audio_sent,
        });
    }

    /// Matches a pong to its ping; unknown, duplicate or stale ids give `None`.
    pub fn pong(
        &mut self,
        id: u32,
        now: Instant,
        server_received: u32,
        server_relayed: u32,
        received: u32,
    ) -> Option<Sample> {
        let pos = self.pending.iter().position(|p| p.id == id)?;
        let p = self.pending[pos];
        self.pending.drain(..=pos); // older pings are lost or late
        Some(Sample {
            at: now,
            rtt_ms: now.saturating_duration_since(p.at).as_secs_f32() * 1000.0,
            counters: Counters {
                sent: p.sent,
                server_received,
                server_relayed,
                received,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: Instant, rtt_ms: f32, counters: Counters) -> Sample {
        Sample {
            at,
            rtt_ms,
            counters,
        }
    }

    #[test]
    fn loss_math() {
        assert_eq!(loss_percent(0, 0), None, "no traffic");
        assert_eq!(loss_percent(100, 100), Some(0.0));
        assert_eq!(loss_percent(100, 90), Some(10.0));
        assert_eq!(
            loss_percent(100, 120),
            Some(0.0),
            "in-flight overshoot clamps to 0"
        );
        assert_eq!(
            loss_percent(5u32.wrapping_sub(10), 0),
            None,
            "backwards jump"
        );
    }

    #[test]
    fn summary_average_jitter_and_loss() {
        let t = Instant::now();
        let mut h = History::default();
        assert_eq!(h.summary(), Summary::default());
        h.push(sample(
            t,
            40.0,
            Counters {
                sent: u32::MAX - 9,
                server_received: 0,
                server_relayed: 0,
                received: 0,
            },
        ));
        h.push(sample(
            t + Duration::from_secs(2),
            60.0,
            Counters::default(),
        ));
        // 100 sent across the u32 wrap, 95 arrived; 50 relayed to us, 40 arrived.
        h.push(sample(
            t + Duration::from_secs(4),
            50.0,
            Counters {
                sent: 90,
                server_received: 95,
                server_relayed: 50,
                received: 40,
            },
        ));
        let s = h.summary();
        assert_eq!(s.last_ms, Some(50.0));
        assert_eq!(s.average_ms, Some(50.0));
        assert_eq!(s.jitter_ms, Some(15.0));
        assert_eq!(s.outbound_loss, Some(5.0));
        assert_eq!(s.inbound_loss, Some(20.0));
    }

    #[test]
    fn history_is_bounded_by_count_and_age() {
        let t = Instant::now();
        let mut h = History::default();
        for i in 0..1000 {
            h.push(sample(
                t + Duration::from_millis(i * 10),
                1.0,
                Counters::default(),
            ));
        }
        assert_eq!(h.len(), MAX_SAMPLES);
        let mut h = History::default();
        for i in 0..200 {
            h.push(sample(
                t + Duration::from_secs(i * 2),
                1.0,
                Counters::default(),
            ));
        }
        assert!(h.len() <= 121, "only the last 4 minutes, got {}", h.len());
        let now = t + Duration::from_secs(398);
        assert!(h
            .points(now)
            .iter()
            .all(|(age, _)| *age <= HISTORY.as_secs_f32()));
    }

    #[test]
    fn pinger_matches_pongs_once() {
        let t = Instant::now();
        let mut p = Pinger::default();
        assert!(p.due(t));
        p.sent(1, t, 10);
        assert!(!p.due(t + Duration::from_secs(1)));
        p.sent(2, t + PING_INTERVAL, 20);
        assert!(p.pong(99, t, 0, 0, 0).is_none(), "unknown id");
        let s = p
            .pong(2, t + PING_INTERVAL + Duration::from_millis(30), 18, 5, 4)
            .unwrap();
        assert!((s.rtt_ms - 30.0).abs() < 0.5);
        assert_eq!(
            s.counters,
            Counters {
                sent: 20,
                server_received: 18,
                server_relayed: 5,
                received: 4
            }
        );
        assert!(p.pong(2, t, 0, 0, 0).is_none(), "duplicate pong");
        assert!(
            p.pong(1, t, 0, 0, 0).is_none(),
            "older ping dropped once a newer one is answered"
        );
        for id in 10..100 {
            p.sent(id, t, 0);
        }
        assert!(p.pending.len() <= MAX_PENDING);
    }
}
