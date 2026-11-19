use serde::{Deserialize, Serialize};

use crate::schema::segment::SegmentSpan;

/// A voice within one file: 0 is the first voice heard, then in order of first appearance.
/// Meaningless across files; each API renders it in its own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
#[schema(value_type = u32)]
pub struct SpeakerId(pub u32);

/// A request's order on telling voices apart: each door decides; a loaded node alone never runs.
/// Asking while the node is off is the door's to refuse — the runner then emits no speakers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diarization {
    Skip,
    Run,
}

/// One stretch of a single voice, in samples from the start of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub span: SegmentSpan,
    pub speaker: SpeakerId,
}
