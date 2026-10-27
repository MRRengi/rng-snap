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

#[cfg(test)]
mod tests {
    use super::*;

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
