//! Audio timeline mixer. Sources drop interleaved stereo f32 samples at the
//! frame position their timestamps say; the encoder pulls 16-bit PCM up to a
//! target position. Gaps (e.g. system sound during silence) become zeros.

use std::collections::VecDeque;

pub struct Mixer {
    /// Frames already handed to the encoder.
    emitted: u64,
    /// Stereo frames from `emitted` onwards.
    buf: VecDeque<[f32; 2]>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer { emitted: 0, buf: VecDeque::new() }
    }

    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Mixes `samples` (interleaved L/R) in starting at frame `pos`.
    /// Anything before what was already emitted is dropped.
    pub fn add(&mut self, pos: i64, samples: &[f32]) {
        let frames = samples.len() / 2;
        for i in 0..frames {
            let at = pos + i as i64;
            if at < self.emitted as i64 {
                continue;
            }
            let idx = (at as u64 - self.emitted) as usize;
            if idx > 48_000 * 30 {
                break; // absurd timestamp, refuse to allocate minutes of silence
            }
            while self.buf.len() <= idx {
                self.buf.push_back([0.0, 0.0]);
            }
            self.buf[idx][0] += samples[i * 2];
            self.buf[idx][1] += samples[i * 2 + 1];
        }
    }

    /// 16-bit interleaved PCM from `emitted` up to `target` (exclusive).
    pub fn take_until(&mut self, target: u64) -> Vec<i16> {
        if target <= self.emitted {
            return Vec::new();
        }
        let n = (target - self.emitted) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for _ in 0..n {
            let f = self.buf.pop_front().unwrap_or([0.0, 0.0]);
            for c in f {
                out.push((c.clamp(-1.0, 1.0) * 32767.0).round() as i16);
            }
        }
        self.emitted = target;
        out
    }
}

/// Keeps one source continuous. Each packet's timestamp, rounded to a frame,
/// wobbles by a sample or so; placing packets by it leaves one-sample gaps
/// and overlaps 100 times a second, which is heard as a buzz. Packets are
/// laid end to end instead, and the timestamp only wins after a reported
/// break or when the stream has drifted further than `slack` frames.
pub struct Track {
    next: Option<i64>,
    slack: i64,
}

impl Track {
    pub fn new(slack: i64) -> Track {
        Track { next: None, slack }
    }

    /// Where a packet of `frames` frames stamped at `stamped` goes.
    pub fn place(&mut self, stamped: i64, frames: usize, discontinuity: bool) -> i64 {
        let pos = match self.next {
            Some(n) if !discontinuity && (stamped - n).abs() <= self.slack => n,
            _ => stamped,
        };
        self.next = Some(pos + frames as i64);
        pos
    }

    /// Forget the position (after a pause the timeline is shifted).
    pub fn reset(&mut self) {
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 440 Hz tone in 10 ms packets whose timestamps wobble by up to
    /// ±0.6 ms: laid end to end it comes out sample-exact; placed by its
    /// stamps (the old way) it is full of clicks.
    fn tone_through(track: bool) -> f32 {
        let rate = 48_000i64;
        let tone = |i: i64| (i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() as f32 * 0.5;
        let packet = 480usize;
        let mut m = Mixer::new();
        let mut t = Track::new(rate / 50);
        let mut seed = 7u64;
        for k in 0..200i64 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let jitter = if k == 0 { 0 } else { ((seed >> 33) % 59) as i64 - 29 };
            let start = k * packet as i64;
            let samples: Vec<f32> = (0..packet as i64).flat_map(|i| [tone(start + i), tone(start + i)]).collect();
            let pos = if track { t.place(start + jitter, packet, false) } else { start + jitter };
            m.add(pos, &samples);
        }
        let out = m.take_until(199 * packet as u64);
        out.chunks_exact(2).enumerate().map(|(i, s)| (s[0] as f32 / 32767.0 - tone(i as i64)).abs()).fold(0.0f32, f32::max)
    }

    #[test]
    fn jittery_packets_stay_seamless() {
        assert!(tone_through(true) < 2.0 / 32767.0, "{}", tone_through(true));
        assert!(tone_through(false) > 0.05, "{}", tone_through(false));
    }

    #[test]
    fn breaks_and_drift_resync() {
        let mut t = Track::new(960);
        assert_eq!(t.place(100, 480, false), 100);
        assert_eq!(t.place(590, 480, false), 580);
        // Device says the stream broke: trust the stamp.
        assert_eq!(t.place(2000, 480, true), 2000);
        // Drifted further than the slack: trust the stamp.
        assert_eq!(t.place(5000, 480, false), 5000);
        t.reset();
        assert_eq!(t.place(9000, 480, false), 9000);
    }

    #[test]
    fn places_by_position_and_fills_gaps() {
        let mut m = Mixer::new();
        m.add(2, &[0.5, -0.5, 0.25, 0.25]);
        let out = m.take_until(5);
        assert_eq!(out, vec![0, 0, 0, 0, 16384, -16384, 8192, 8192, 0, 0]);
        assert_eq!(m.emitted(), 5);
    }

    #[test]
    fn mixes_two_sources_and_clamps() {
        let mut m = Mixer::new();
        m.add(0, &[0.5, 0.5, 0.9, 0.9]);
        m.add(0, &[0.25, 0.0, 0.9, -0.1]);
        assert_eq!(m.take_until(2), vec![24575, 16384, 32767, 26214]);
    }

    #[test]
    fn late_samples_are_dropped() {
        let mut m = Mixer::new();
        m.take_until(10);
        m.add(8, &[1.0, 1.0, 1.0, 1.0, 0.5, 0.5]);
        assert_eq!(m.take_until(11), vec![16384, 16384]);
    }

    #[test]
    fn nothing_before_target() {
        let mut m = Mixer::new();
        m.take_until(4);
        assert!(m.take_until(3).is_empty());
    }
}
