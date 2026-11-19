use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use e_voice_core::schema::error::NodeError;
use e_voice_core::schema::lang::Lang;
use serde::{Deserialize, Serialize};

use crate::schema::emotion::Emotion;
use crate::schema::segment::{SegmentId, SegmentSpan};
use crate::schema::speaker::{SpeakerId, Turn};

/// Everything a stream emits to its consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    Wake(WakeEvent),
    Speech(SpeechEvent),
    Partial(PartialEvent),
    Final(FinalEvent),
    Speakers(SpeakersEvent),
    Closed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WakeEvent {
    pub keyword: String,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpeechState {
    Started,
    Stopped,
}

/// Voice activity of one segment: `at` is the stream sample where speech started (estimated) or
/// stopped (exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpeechEvent {
    pub segment: SegmentId,
    pub state: SpeechState,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PartialEvent {
    pub segment: SegmentId,
    pub text: String,
}

/// Exactly one per segment; `error` is set only when the required ASR node failed. `speaker` exists
/// only on diarized uploads and is omitted from the JSON otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FinalEvent {
    pub segment: SegmentId,
    pub span: SegmentSpan,
    pub lang: Lang,
    pub text: String,
    pub emotion: Emotion,
    pub error: Option<NodeError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<SpeakerId>,
}

/// One segment's voice in a diarized upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SegmentSpeaker {
    pub segment: SegmentId,
    pub speaker: SpeakerId,
}

/// Once per diarized upload, after its last final and before `Closed`: the voice of each segment,
/// in segment order; a segment no voice overlaps is absent. Never on live audio, nor while the node
/// is off. `error` set means no assignment was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpeakersEvent {
    pub segments: Vec<SegmentSpeaker>,
    pub error: Option<NodeError>,
}

impl SpeakersEvent {
    // ##### PRIVATE #####

    /// The voice overlapping `span` longest (ties: the lowest backend id) among turns from `first`.
    fn join_majority(turns: &[Turn], first: usize, span: SegmentSpan) -> Option<SpeakerId> {
        let mut overlaps: BTreeMap<SpeakerId, u64> = BTreeMap::new();
        turns
            .iter()
            .skip(first)
            .take_while(|turn| turn.span.start < span.end)
            .map(|turn| {
                let overlap = span
                    .end
                    .min(turn.span.end)
                    .saturating_sub(span.start.max(turn.span.start));
                (turn.speaker, overlap)
            })
            .filter(|(_, overlap)| *overlap > 0)
            .for_each(|(speaker, overlap)| {
                let total = overlaps.entry(speaker).or_default();
                *total = total.saturating_add(overlap);
            });
        overlaps
            .into_iter()
            .max_by(|(a, x), (b, y)| x.cmp(y).then(b.cmp(a)))
            .map(|(speaker, _)| speaker)
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Segments and turns, both sorted by start, swept once: O(segments + turns) plus each
    /// overlapping pair (only a turn ended behind a longer one still open is scanned again). Ids are
    /// renumbered in order of first appearance, so the first voice heard is 0.
    #[must_use]
    pub fn join(segments: &[(SegmentId, SegmentSpan)], turns: &[Turn]) -> Self {
        let mut first = 0usize;
        let mut order: BTreeMap<SpeakerId, SpeakerId> = BTreeMap::new();
        let segments = segments
            .iter()
            .filter_map(|&(segment, span)| {
                first = first.saturating_add(
                    turns
                        .iter()
                        .skip(first)
                        .take_while(|turn| turn.span.end <= span.start)
                        .count(),
                );
                let raw = Self::join_majority(turns, first, span)?;
                let next = SpeakerId(u32::try_from(order.len()).unwrap_or(u32::MAX));
                let speaker = match order.entry(raw) {
                    Entry::Occupied(seen) => *seen.get(),
                    Entry::Vacant(new) => *new.insert(next),
                };
                Some(SegmentSpeaker { segment, speaker })
            })
            .collect();
        Self { segments, error: None }
    }

    #[must_use]
    pub fn failed(error: NodeError) -> Self {
        Self {
            segments: Vec::new(),
            error: Some(error),
        }
    }
}
