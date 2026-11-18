use std::fmt::{self, Debug};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use e_voice_core::runtime::Runtime;
use e_voice_core::schema::error::{BackendError, DiarizeStage, NodeError};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig, OfflineSpeakerDiarizationSegment,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};

use crate::config::diarize::{DiarizeConfig, DiarizePrecision};
use crate::schema::audio::RATE;
use crate::schema::segment::SegmentSpan;
use crate::schema::speaker::{SpeakerId, Turn};
use crate::workflow::diarize::base::Diarize;
use crate::workflow::parts::Parts;

/// sherpa-onnx offline diarization: pyannote segmentation, embeddings, fast clustering. One file at
/// a time: the lock keeps concurrent uploads inside this node's own thread budget.
pub struct SherpaDiarize {
    diarizer: Mutex<OfflineSpeakerDiarization>,
}

impl Debug for SherpaDiarize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaDiarize").finish_non_exhaustive()
    }
}

impl SherpaDiarize {
    // ##### PRIVATE #####

    /// The segmentation weights by name, not Parts' int8-first rule: fp32 is the default because the
    /// reference DER was measured on it; int8 is a bench arm.
    const fn new_segmentation(precision: DiarizePrecision) -> &'static str {
        match precision {
            DiarizePrecision::Fp32 => "model.onnx",
            DiarizePrecision::Int8 => "model.int8.onnx",
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn diarize_sample(seconds: f32) -> u64 {
        (f64::from(seconds).max(0.0) * f64::from(RATE)).round() as u64
    }

    fn diarize_turn(segment: &OfflineSpeakerDiarizationSegment) -> Option<Turn> {
        Some(Turn {
            span: SegmentSpan {
                start: Self::diarize_sample(segment.start),
                end: Self::diarize_sample(segment.end),
            },
            speaker: SpeakerId(u32::try_from(segment.speaker).ok()?),
        })
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// # Errors
    /// Model files missing from either directory, the runtime rejecting them, or a model expecting a
    /// rate other than [`RATE`].
    pub fn new(segmentation: &Path, embedding: &Path, config: &DiarizeConfig) -> Result<Self, BackendError> {
        let threads = i32::from(config.threads);
        let weights = segmentation.join(Self::new_segmentation(config.precision));
        let sherpa = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(
                        weights
                            .is_file()
                            .then_some(weights.display().to_string())
                            .ok_or_else(|| BackendError::Missing(weights.clone()))?,
                    ),
                    ..OfflineSpeakerSegmentationPyannoteModelConfig::default()
                },
                num_threads: threads,
                debug: false,
                provider: Some(Runtime::provider()),
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: Some(Parts::onnx(embedding, "")?),
                num_threads: threads,
                debug: false,
                provider: Some(Runtime::provider()),
            },
            clustering: FastClusteringConfig {
                num_clusters: config.speakers.map_or(-1, i32::from),
                threshold: config.threshold,
                compute_confidence: false,
            },
            min_duration_on: config.min_on.as_secs_f32(),
            min_duration_off: config.min_off.as_secs_f32(),
        };
        let diarizer = OfflineSpeakerDiarization::create(&sherpa)
            .filter(|diarizer| diarizer.sample_rate() == RATE.cast_signed())
            .ok_or(BackendError::Load("sherpa diarize"))?;
        Ok(Self {
            diarizer: Mutex::new(diarizer),
        })
    }
}

impl Diarize for SherpaDiarize {
    fn diarize(&self, audio: &[f32]) -> Result<Vec<Turn>, NodeError> {
        let result = self
            .diarizer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .process(audio)
            .ok_or(NodeError::Diarize(DiarizeStage::Process))?;
        let segments = result.sort_by_start_time();
        // An empty read-back of a non-empty result is the runtime failing, not silence.
        let expected = usize::try_from(result.num_segments()).unwrap_or(0);
        (segments.len() == expected)
            .then(|| segments.iter().map(Self::diarize_turn).collect::<Option<Vec<Turn>>>())
            .flatten()
            .ok_or(NodeError::Diarize(DiarizeStage::Segments))
    }
}
