//! Writers: a [`Program`] as source text for another API. A writer that cannot express a program refuses it with the
//! reason; it never approximates.

mod pipeline;

pub use pipeline::{ClipVaryings, Pipeline, Written, wgsl_for};

use crate::{Program, ProgramError as Error};

/// The program as WGSL, by naga's WGSL backend. The program is validated first; a program the validator or the
/// backend refuses is an error.
///
/// The conventions are not applied: the text is the program as written for its source API (D3D clip depth, texture
/// origin), and a pipeline built from it states them where it meets the pipeline. Behaviours of D3D the body spells
/// out ([`crate::D3dModes`]) are plain operations in the text; the denormal flush is the source's expectation, which
/// WGSL leaves to the implementation.
///
/// What the text holds is stated beside it, so a consumer never reads it back: where each varying is (its location
/// and interpolation, the clip distances' one member) is [`crate::entry_io`]'s, which resources it declares and never
/// uses [`Program::unread_resources`], and which constant words it reads [`Program::constant_reads`].
///
/// A program of a stage WGSL has no entry point for (a hull, domain or geometry program, a vertex program that feeds a
/// tessellator) is refused by name: a consumer that tessellates writes its own program from these (see
/// [`wgsl_functions`] for the functions as text).
pub fn wgsl(program: &Program) -> Result<String, Error> {
    writable(program)?;
    wgsl_functions(program)
}

/// Whether WGSL has an entry point for the program's stage ([`wgsl`]'s refusal).
fn writable(program: &Program) -> Result<(), Error> {
    if crate::stage_functions(&program.interface)
        .map_err(Error::Invalid)?
        .is_some()
    {
        return Err(Error::Unsupported {
            writer: "wgsl",
            detail: format!(
                "a {:?} program{} has no WGSL stage: it is functions, not an entry point",
                program.stage,
                if program.stage == crate::Stage::Vertex {
                    " that feeds a tessellator"
                } else {
                    ""
                }
            ),
        });
    }
    if program.stage.is_ray_tracing() {
        return Err(ray_tracing(program));
    }
    Ok(())
}

/// The refusal of a ray tracing program's WGSL.
fn ray_tracing(program: &Program) -> Error {
    Error::Unsupported {
        writer: "wgsl",
        detail: format!(
            "a {:?} program has no WGSL form: WebGPU has no ray tracing pipeline, and the program's operations \
             ({}) are the pipeline's",
            program.stage,
            crate::rt::OPERATIONS.join(", ")
        ),
    }
}

/// The module as WGSL whatever its stage: for a program of a stage with no entry point, its functions, to be read.
pub fn wgsl_functions(program: &Program) -> Result<String, Error> {
    if program.stage.is_ray_tracing() {
        return Err(ray_tracing(program));
    }
    module_text(program)
}

/// The module as WGSL text whatever its stage, for a program's text form: a ray tracing program too, which no consumer
/// is offered as WGSL ([`wgsl_functions`] refuses it).
pub(crate) fn module_text(program: &Program) -> Result<String, Error> {
    let info = program.validated()?;
    naga::back::wgsl::write_string(&program.module, &info, naga::back::wgsl::WriterFlags::empty()).map_err(
        |e| Error::Unsupported {
            writer: "wgsl",
            detail: e.to_string(),
        },
    )
}
