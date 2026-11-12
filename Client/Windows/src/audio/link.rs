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
/// No pong for this long: the numbers are stale (UDP blocked or lost).
pub const STALE_AFTER: Duration = Duration::from_secs(8);

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
    /// Seconds since the last pong once that exceeds [`STALE_AFTER`].
    pub stale_secs: Option<f32>,
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

    /// Stats over the samples within [`HISTORY`] of `now`. `since` is when
    /// the session started, so a session that never got a pong turns stale too.
    pub fn summary(&self, now: Instant, since: Instant) -> Summary {
        let recent: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| now.saturating_duration_since(s.at) <= HISTORY)
            .collect();
        let last_at = self.samples.back().map_or(since, |s| s.at);
        let silent = now.saturating_duration_since(last_at);
        let stale_secs = (silent > STALE_AFTER).then_some(silent.as_secs_f32());
        let (Some(first), Some(last)) = (recent.first(), recent.last()) else {
            return Summary {
                stale_secs,
                ..Summary::default()
            };
        };
        let n = recent.len() as f32;
        let average = recent.iter().map(|s| s.rtt_ms).sum::<f32>() / n;
        let jitter = (recent.len() > 1).then(|| {
            let diffs: f32 = recent
                .iter()
                .zip(recent.iter().skip(1))
                .map(|(a, b)| (b.rtt_ms - a.rtt_ms).abs())
                .sum();
            diffs / (n - 1.0)
        });
        let (a, b) = (first.counters, last.counters);
        Summary {
            stale_secs,
            last_ms: if stale_secs.is_some() {
                None
            } else {
                Some(last.rtt_ms)
            },
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

/// Replay window of one UDP session. `id` changes on every new session so a
/// receive thread of an old session can never touch the new window.
#[derive(Debug)]
struct SessionReplay {
    id: u64,
    started: Instant,
    window: ReplayWindow,
}

impl Default for SessionReplay {
    fn default() -> Self {
        Self {
            id: 0,
            started: Instant::now(),
            window: ReplayWindow::default(),
        }
    }
}

/// Shared between the audio threads and the UI (lives in `AudioControl`).
/// Lock order: `replay` before `history`.
#[derive(Debug, Default)]
pub struct LinkStats {
    /// Audio packets sent (voice + screen audio).
    pub audio_sent: AtomicU32,
    /// Authenticated audio packets received.
    pub audio_received: AtomicU32,
    next_ping_id: AtomicU32,
    /// Survives engine restarts (device change) within one session.
    replay: Mutex<SessionReplay>,
    history: Mutex<History>,
}

impl LinkStats {
    /// New session (new key, server counters start at 0 again).
    pub fn reset(&self) {
        let mut r = self.replay.lock().unwrap();
        r.id = r.id.wrapping_add(1);
        r.started = Instant::now();
        r.window = ReplayWindow::default();
        self.history.lock().unwrap().clear();
        self.audio_sent.store(0, Ordering::Relaxed);
        self.audio_received.store(0, Ordering::Relaxed);
    }

    /// Current session id; a receive thread takes it once at start.
    pub fn session(&self) -> u64 {
        self.replay.lock().unwrap().id
    }

    /// Replay check for `session`. `None`: that session is over, the caller must stop.
    pub fn accept(&self, session: u64, counter: u64) -> Option<bool> {
        let mut r = self.replay.lock().unwrap();
        (r.id == session).then(|| r.window.accept(counter))
    }

    pub fn next_ping_id(&self) -> u32 {
        self.next_ping_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Ignored when `session` is no longer the current one.
    pub fn record(&self, session: u64, s: Sample) {
        let r = self.replay.lock().unwrap();
        if r.id == session {
            self.history.lock().unwrap().push(s);
        }
    }

    pub fn view(&self, now: Instant) -> (Vec<(f32, f32)>, Summary) {
        let r = self.replay.lock().unwrap();
        let h = self.history.lock().unwrap();
        (h.points(now), h.summary(now, r.started))
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
        assert_eq!(h.summary(t, t), Summary::default());
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
        let s = h.summary(t + Duration::from_secs(5), t);
        assert_eq!(s.stale_secs, None);
        assert_eq!(s.last_ms, Some(50.0));
        assert_eq!(s.average_ms, Some(50.0));
        assert_eq!(s.jitter_ms, Some(15.0));
        assert_eq!(s.outbound_loss, Some(5.0));
        assert_eq!(s.inbound_loss, Some(20.0));
    }

    #[test]
    fn summary_goes_stale_without_pongs() {
        let t = Instant::now();
        let mut h = History::default();
        let silent = h.summary(t + STALE_AFTER + Duration::from_secs(1), t);
        assert!(silent.stale_secs.is_some(), "never answered");
        for i in 0..5 {
            h.push(sample(
                t + Duration::from_secs(i * 2),
                6.0,
                Counters::default(),
            ));
        }
        let last = t + Duration::from_secs(8);
        assert_eq!(
            h.summary(last + Duration::from_secs(1), t).last_ms,
            Some(6.0)
        );
        let s = h.summary(last + STALE_AFTER + Duration::from_secs(1), t);
        assert!(s.stale_secs.is_some());
        assert_eq!(s.last_ms, None, "no fresh ping to show");
        let s = h.summary(last + HISTORY + Duration::from_secs(1), t);
        assert_eq!(s.average_ms, None, "old samples drop out of the summary");
        assert_eq!(s.outbound_loss, None);
    }

    #[test]
    fn old_session_cannot_touch_new_replay_window() {
        let link = LinkStats::default();
        let old = link.session();
        assert_eq!(link.accept(old, 5), Some(true));
        link.reset();
        let new = link.session();
        assert_ne!(old, new);
        // A late packet of the old receive thread with a huge counter.
        assert_eq!(link.accept(old, 1_000_000), None, "old thread must stop");
        assert_eq!(link.accept(new, 1), Some(true), "new session starts fresh");
        link.record(old, sample(Instant::now(), 1.0, Counters::default()));
        assert!(link.view(Instant::now()).0.is_empty(), "old sample ignored");
        link.record(new, sample(Instant::now(), 1.0, Counters::default()));
        assert_eq!(link.view(Instant::now()).0.len(), 1);
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
