use std::collections::VecDeque;
use std::num::NonZeroU32;

use e_voice_core::audio::{AudioEncoding, AudioError, AudioIngest};

/// A source rate and the device rate it is resampled to.
type Pair = (u32, u32);

/// Mono samples queued at the device rate, resampled by core's streaming FFT resampler. Equal rates pass
/// through untouched. The resampler holds back up to one input chunk, so a stream ends with
/// [`PlaybackQueue::finish`] to play its tail; a new rate pair finishes the old one first.
#[derive(Debug, Default)]
pub struct PlaybackQueue {
    samples: VecDeque<f32>,
    ingest: Option<(Pair, AudioIngest)>,
}

impl PlaybackQueue {
    // ##### PRIVATE #####

    /// Plays out what `held` still keeps.
    fn common_drain(&mut self, held: Option<(Pair, AudioIngest)>) -> Result<(), AudioError> {
        let tail = held.map(|(_, mut ingest)| ingest.flush()).transpose()?;
        self.samples.extend(tail.unwrap_or_default());
        Ok(())
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Appends `samples` recorded at `rate` for a device playing at `target`.
    ///
    /// # Errors
    /// The resampler cannot convert between the two rates, or fails.
    pub fn push(&mut self, samples: &[f32], rate: NonZeroU32, target: u32) -> Result<(), AudioError> {
        let pair = (rate.get(), target.max(1));
        let current = match self.ingest.take() {
            Some((held, ingest)) if held == pair => ingest,
            stale => {
                self.common_drain(stale)?;
                AudioIngest::new(pair.0, pair.1, AudioEncoding::F32le)?
            }
        };
        let (_, ingest) = self.ingest.insert((pair, current));
        let resampled = ingest.feed(samples)?;
        self.samples.extend(resampled);
        Ok(())
    }

    /// Ends the stream: the resampler's tail joins the queue and the next push starts afresh.
    ///
    /// # Errors
    /// The resampler fails on its last chunk.
    pub fn finish(&mut self) -> Result<(), AudioError> {
        let held = self.ingest.take();
        self.common_drain(held)
    }

    /// The next sample to play.
    pub fn pop(&mut self) -> Option<f32> {
        self.samples.pop_front()
    }

    /// Drops every queued sample and whatever the resampler still holds (barge-in).
    pub fn clear(&mut self) {
        self.samples.clear();
        self.ingest = None;
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
