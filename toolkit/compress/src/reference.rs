//! oozextract, a second Kraken decoder to hold the game's Oodle library to, behind one boundary: its refusals and its panics
//! (its decoder asserts where it should refuse) both come back as a [`Failure`], so a corrupt or unsupported stream is a named
//! error and never a panic inside a gate. A test aid: enable the `reference-decoder` feature from a dev-dependency.

use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Debug, thiserror::Error)]
pub enum Failure {
    #[error("oozextract refused the stream: {0}")]
    Refused(oozextract::OozError),
    #[error("oozextract panicked instead of refusing the stream (an upstream defect): {0}")]
    Panicked(String),
    #[error("oozextract wrote {written} bytes, expected {expected}")]
    Short { written: usize, expected: usize },
}

impl Failure {
    /// Whether this is a refusal for `reason`: oozextract wraps its error once per layer it passes through, each wrapper
    /// without a message of its own, so the reason is that of the innermost error that states one.
    pub fn refuses_for(&self, reason: &str) -> bool {
        let Failure::Refused(e) = self else {
            return false;
        };
        let mut at: Option<&(dyn std::error::Error + 'static)> = Some(e);
        while let Some(error) = at {
            if error
                .downcast_ref::<oozextract::OozError>()
                .is_some_and(|ooz| ooz.message.as_deref() == Some(reason))
            {
                return true;
            }
            at = error.source();
        }
        false
    }
}

/// The `expected` bytes oozextract decodes from `input`.
pub fn decode(input: &[u8], expected: usize) -> Result<Vec<u8>, Failure> {
    let mut out = vec![0; expected];
    let written = catch_unwind(AssertUnwindSafe(|| {
        oozextract::Extractor::new().read_from_slice(input, &mut out)
    }))
    .map_err(|payload| {
        Failure::Panicked(
            payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".to_owned()),
        )
    })?
    .map_err(Failure::Refused)?;
    if written != expected {
        return Err(Failure::Short { written, expected });
    }
    Ok(out)
}
