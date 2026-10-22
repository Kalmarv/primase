//! Writers: a [`Program`] as source text for another API. A writer that cannot express a program refuses it with the
//! reason; it never approximates.

use crate::{Program, ProgramError as Error};

/// The program as WGSL, by naga's WGSL backend. The program is validated first; a program the validator or the
/// backend refuses is an error.
///
/// The conventions are not applied: the text is the program as written for its source API (D3D clip depth, texture
/// origin), and a pipeline built from it states them where it meets the pipeline. Behaviours of D3D the body spells
/// out ([`crate::D3dModes`]) are plain operations in the text; the denormal flush is the source's expectation, which
/// WGSL leaves to the implementation.
pub fn wgsl(program: &Program) -> Result<String, Error> {
    let info = program.validate().map_err(|problems| {
        Error::Invalid(
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    naga::back::wgsl::write_string(&program.module, &info, naga::back::wgsl::WriterFlags::empty()).map_err(
        |e| Error::Unsupported {
            writer: "wgsl",
            detail: e.to_string(),
        },
    )
}
