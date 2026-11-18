use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiarizeBackend {
    #[default]
    Off,
    /// sherpa-onnx offline diarization: pyannote segmentation 3.0, speaker embeddings, clustering.
    Sherpa,
}

/// The speaker-embedding model the clustering compares voices with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiarizeEmbedding {
    /// WeSpeaker ResNet34-LM, trained on VoxCeleb.
    #[default]
    #[serde(rename = "wespeaker-resnet34-lm")]
    WespeakerResnet34Lm,
    /// 3D-Speaker CAM++, zh/en common (advanced).
    #[serde(rename = "campplus-zh-en")]
    CampplusZhEn,
    /// 3D-Speaker `ERes2Net`, trained on VoxCeleb like the default.
    #[serde(rename = "eres2net-voxceleb")]
    Eres2netVoxceleb,
}

/// Weights of the segmentation model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiarizePrecision {
    #[default]
    Fp32,
    Int8,
}

/// Speaker diarization of uploaded files, run once over the whole file beside transcription; `off`
/// leaves every segment without a speaker. `speakers` fixes the count and then `threshold` is
/// ignored. `threads` is this node's own budget, apart from `jobs`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiarizeConfig {
    pub backend: DiarizeBackend,
    pub embedding: DiarizeEmbedding,
    pub precision: DiarizePrecision,
    pub threads: u16,
    pub threshold: f32,
    pub speakers: Option<u16>,
    #[serde(with = "humantime_serde")]
    pub min_on: Duration,
    #[serde(with = "humantime_serde")]
    pub min_off: Duration,
}

impl Default for DiarizeConfig {
    fn default() -> Self {
        Self {
            backend: DiarizeBackend::Off,
            embedding: DiarizeEmbedding::WespeakerResnet34Lm,
            precision: DiarizePrecision::Fp32,
            threads: 4,
            threshold: 0.5,
            speakers: None,
            min_on: Duration::from_millis(300),
            min_off: Duration::from_millis(500),
        }
    }
}

impl DiarizeConfig {
    /// Segmentation then embedding model; `None` while off.
    #[must_use]
    pub const fn models(&self) -> Option<(&'static str, &'static str)> {
        match self.backend {
            DiarizeBackend::Off => None,
            DiarizeBackend::Sherpa => Some((
                "pyannote-segmentation-3.0",
                match self.embedding {
                    DiarizeEmbedding::WespeakerResnet34Lm => "wespeaker-resnet34-lm",
                    DiarizeEmbedding::CampplusZhEn => "campplus-zh-en-advanced",
                    DiarizeEmbedding::Eres2netVoxceleb => "eres2net-en-voxceleb",
                },
            )),
        }
    }
}
