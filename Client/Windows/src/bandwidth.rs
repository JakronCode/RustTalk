//! Local application payload accounting. Fixed counters; no payloads retained.
use rusttalk_shared::{
    audio_packet::{AudioHeader, FLAG_CONTROL, FLAG_SCREEN_AUDIO},
    protocol::{ClientMessage, ServerMessage, VideoKind},
};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const MEDIA: usize = 5;
pub const HISTORY: Duration = Duration::from_secs(240);
pub type Counts = [[u64; MEDIA]; 2];
pub type Rates = [[f64; MEDIA]; 2];
pub const IN: usize = 0;
pub const OUT: usize = 1;
#[derive(Clone, Copy)]
pub enum Media {
    Screen,
    Camera,
    Voice,
    Shared,
    Other,
}
impl Media {
    fn video(kind: VideoKind) -> Self {
        match kind {
            VideoKind::Screen => Self::Screen,
            VideoKind::Camera => Self::Camera,
        }
    }
    pub fn audio(h: &AudioHeader) -> Self {
        if h.flags & FLAG_CONTROL != 0 {
            Self::Other
        } else if h.stream_id != 0 || h.flags & FLAG_SCREEN_AUDIO != 0 {
            Self::Shared
        } else {
            Self::Voice
        }
    }
}
#[derive(Default, Debug)]
pub struct Traffic(Mutex<Counts>);
impl Traffic {
    pub fn add(&self, direction: usize, media: Media, bytes: usize) {
        let mut counts = self.0.lock().unwrap();
        let n = &mut counts[direction][media as usize];
        *n = n.saturating_add(bytes as u64);
    }
    pub fn snapshot(&self) -> Counts {
        *self.0.lock().unwrap()
    }
    pub fn sent(&self, msg: &ClientMessage) {
        let (kind, len) = match msg {
            ClientMessage::VideoFrame { kind, data } => (Media::video(*kind), data.len()),
            _ => (
                Media::Other,
                rusttalk_shared::framing::frame_len(msg).unwrap_or(0),
            ),
        };
        self.add(OUT, kind, len);
    }
    pub fn received(&self, msg: &ServerMessage, serialized_len: usize) {
        let (kind, len) = match msg {
            ServerMessage::VideoFrame { kind, data, .. } => (Media::video(*kind), data.len()),
            _ => (Media::Other, serialized_len),
        };
        self.add(IN, kind, len);
    }
}
fn difference(a: Counts, b: Counts) -> Counts {
    std::array::from_fn(|d| std::array::from_fn(|m| a[d][m].saturating_sub(b[d][m])))
}
pub struct Call {
    baseline: Counts,
    previous: Counts,
    at: Instant,
    pub totals: Counts,
    pub rates: Rates,
    pub history: VecDeque<(Instant, Rates)>,
}
impl Call {
    pub fn new(now: Instant, counts: Counts) -> Self {
        Self {
            baseline: counts,
            previous: counts,
            at: now,
            totals: [[0; MEDIA]; 2],
            rates: [[0.; MEDIA]; 2],
            history: VecDeque::new(),
        }
    }
    pub fn sample(&mut self, now: Instant, counts: Counts) {
        self.totals = difference(counts, self.baseline);
        let elapsed = now.saturating_duration_since(self.at);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let delta = difference(counts, self.previous);
        self.rates = std::array::from_fn(|d| {
            std::array::from_fn(|m| delta[d][m] as f64 / elapsed.as_secs_f64())
        });
        self.previous = counts;
        self.at = now;
        self.history.push_back((now, self.rates));
        while self.history.len() > 240
            || self
                .history
                .front()
                .is_some_and(|(t, _)| now.saturating_duration_since(*t) > HISTORY)
        {
            self.history.pop_front();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_payloads_and_directions() {
        let t = Traffic::default();
        t.sent(&ClientMessage::VideoFrame {
            kind: VideoKind::Screen,
            data: vec![0; 11],
        });
        t.received(
            &ServerMessage::VideoFrame {
                from_id: 4,
                kind: VideoKind::Camera,
                data: vec![0; 7],
            },
            100,
        );
        let mut h = AudioHeader::new(1, 0, 0);
        t.add(IN, Media::audio(&h), 3);
        h.flags = FLAG_SCREEN_AUDIO;
        t.add(OUT, Media::audio(&h), 5);
        h.flags = 0;
        h.stream_id = u32::MAX;
        t.add(IN, Media::audio(&h), 9);
        h.flags = FLAG_CONTROL;
        t.add(IN, Media::audio(&h), 2);
        assert_eq!(t.snapshot(), [[0, 7, 3, 9, 2], [11, 0, 0, 5, 0]]);
    }
    #[test]
    fn baseline_rates_reset_and_bounds() {
        let now = Instant::now();
        let mut counts = [[10; MEDIA]; 2];
        let mut call = Call::new(now, counts);
        counts[OUT][0] += 100;
        call.sample(now, counts);
        assert_eq!(call.totals[OUT][0], 100);
        assert!(call.history.is_empty());
        call.sample(now + Duration::from_secs(2), counts);
        assert_eq!(call.rates[OUT][0], 50.);
        for s in 3..1000 {
            call.sample(now + Duration::from_secs(s), counts);
        }
        assert_eq!(call.history.len(), 240);
        assert_eq!(call.rates, [[0.; MEDIA]; 2]);
        call.sample(now + Duration::from_secs(2000), counts);
        assert_eq!(call.history.len(), 1);
        let reset = Call::new(now, counts);
        assert_eq!(reset.totals, [[0; MEDIA]; 2]);
        assert!(reset.history.is_empty());
    }
}
