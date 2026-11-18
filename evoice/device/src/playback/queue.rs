use std::collections::VecDeque;
use std::num::NonZeroU32;

/// Mono samples queued at the device rate, resampled linearly from any source rate (a tester, not a
/// mixer). The fractional read position and the previous source sample carry across pushes.
#[derive(Debug, Default)]
pub struct PlaybackQueue {
    samples: VecDeque<f32>,
    phase: f64,
    last: f32,
}

impl PlaybackQueue {
    /// Appends `samples` recorded at `rate` for a device playing at `target`. A zero source rate would
    /// never advance the read position, so the type refuses it.
    #[allow(clippy::cast_possible_truncation)]
    pub fn push(&mut self, samples: &[f32], rate: NonZeroU32, target: u32) {
        let step = f64::from(rate.get()) / f64::from(target.max(1));
        let (mut phase, mut last) = (self.phase, self.last);
        for &sample in samples {
            while phase < 1.0 {
                let mixed = last + (sample - last) * phase as f32;
                self.samples.push_back(mixed);
                phase += step;
            }
            phase -= 1.0;
            last = sample;
        }
        (self.phase, self.last) = (phase, last);
    }

    /// The next sample to play.
    pub fn pop(&mut self) -> Option<f32> {
        self.samples.pop_front()
    }

    /// Drops every queued sample; the resampling phase carries on.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}
