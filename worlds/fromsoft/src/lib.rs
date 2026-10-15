//! Read-only Sekiro BHD5/BDT archives, DCX envelopes and BND4 binders.
pub mod archive;
pub mod bhd;
pub mod binder;
mod bytes;
pub mod dcx;
mod keys;
pub mod msb;
pub use archive::{Archive, NameDictionary};
pub use binder::{Binder, BinderFile};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: usize,
        detail: String,
    },
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("compression: {0}")]
    Compression(#[from] primase_compress::Error),
    #[error("entry hash {0:08x} not found")]
    Missing(u32),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn invalid(format: &'static str, offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}
pub mod tpf;
