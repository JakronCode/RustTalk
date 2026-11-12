//! Per-session UDP bookkeeping: replay protection, ping rate limit and the
//! packet counters reported in pongs. Fixed size, RAM only, never logged.

use std::time::Instant;

use rusttalk_shared::secure::ReplayWindow;

/// Pings answered in a burst.
const PING_BURST: f32 = 3.0;
/// Sustained pings per second (the client sends one every 2 s).
const PING_PER_SEC: f32 = 1.0;

#[derive(Debug)]
pub struct UdpLink {
    replay: ReplayWindow,
    ping_tokens: f32,
    ping_refill: Instant,
    /// Authenticated audio packets from this client (wraps).
    pub audio_received: u32,
    /// Audio packets relayed to this client (wraps).
    pub audio_relayed: u32,
}

impl UdpLink {
    pub fn new(now: Instant) -> Self {
        Self {
            replay: ReplayWindow::default(),
            ping_tokens: PING_BURST,
            ping_refill: now,
            audio_received: 0,
            audio_relayed: 0,
        }
    }

    /// Replay check for an authenticated packet counter.
    pub fn accept(&mut self, counter: u64) -> bool {
        self.replay.accept(counter)
    }

    /// Token bucket: `true` = answer this ping.
    pub fn allow_ping(&mut self, now: Instant) -> bool {
        let elapsed = now
            .saturating_duration_since(self.ping_refill)
            .as_secs_f32();
        self.ping_refill = now;
        self.ping_tokens = (self.ping_tokens + elapsed * PING_PER_SEC).min(PING_BURST);
        if self.ping_tokens >= 1.0 {
            self.ping_tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn ping_rate_is_limited_and_refills() {
        let t0 = Instant::now();
        let mut l = UdpLink::new(t0);
        let burst = (0..20).filter(|_| l.allow_ping(t0)).count();
        assert_eq!(burst, 3);
        assert!(!l.allow_ping(t0 + Duration::from_millis(500)));
        assert!(l.allow_ping(t0 + Duration::from_millis(1100)));
        // A long pause never grows the bucket beyond the burst.
        let later = t0 + Duration::from_secs(3600);
        assert_eq!((0..20).filter(|_| l.allow_ping(later)).count(), 3);
    }

    #[test]
    fn replays_are_rejected() {
        let mut l = UdpLink::new(Instant::now());
        assert!(l.accept(1));
        assert!(!l.accept(1));
    }
}
