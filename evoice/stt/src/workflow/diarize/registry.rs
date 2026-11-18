use std::sync::Arc;

use e_voice_core::models::ModelStore;
use e_voice_core::schema::error::BackendError;

use crate::config::diarize::DiarizeConfig;
use crate::workflow::diarize::base::Diarize;
use crate::workflow::diarize::sherpa::SherpaDiarize;

/// Configuration → diarization backend, or none when disabled.
#[derive(Debug)]
pub struct DiarizeRegistry;

impl DiarizeRegistry {
    /// # Errors
    /// A model is not in the manifest, or the backend cannot be built from its files.
    pub fn build(config: &DiarizeConfig, store: &ModelStore) -> Result<Option<Arc<dyn Diarize>>, BackendError> {
        let Some((segmentation, embedding)) = config.models() else {
            return Ok(None);
        };
        let dir = |model: &str| store.dir(model).map_err(|_| BackendError::Unknown(model.to_owned()));
        Ok(Some(Arc::new(SherpaDiarize::new(
            &dir(segmentation)?,
            &dir(embedding)?,
            config,
        )?)))
    }
}
