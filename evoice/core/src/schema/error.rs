use serde::{Deserialize, Serialize};

/// Why a node produced no result for a segment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", content = "detail", rename_all = "lowercase")]
pub enum NodeError {
    #[error("deadline exceeded")]
    Timeout,
    #[error("segment stayed open beyond the stall limit")]
    Stalled,
    #[error("dropped under load")]
    Overload,
    #[error("cancelled")]
    Cancelled,
    #[error("backend failure: {0}")]
    Backend(String),
    #[error("diarization failed at {0}")]
    Diarize(DiarizeStage),
}

/// Where a whole-file diarization failed; the runtime reports no cause beyond the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiarizeStage {
    /// Segmentation, embedding or clustering returned no result.
    Process,
    /// A result was produced but its segments could not be read back.
    Segments,
}

impl std::fmt::Display for DiarizeStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Process => "process",
            Self::Segments => "segments",
        })
    }
}

/// Why a backend or one of its sessions could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    #[error("model {0} is not in the manifest")]
    Unknown(String),
    #[error("model file not found: {0}")]
    Missing(std::path::PathBuf),
    #[error("{0} rejected its configuration")]
    Load(&'static str),
    #[error("every {0} worker is busy")]
    Busy(&'static str),
}
