//! The SDB2 shader database (`anvil_formats::shaunwhite::shaders`) with each stage program framed: a database whose
//! programs are not each one complete SM3 program of its stated stage is refused.

use anvil_formats::shaunwhite::shaders::{self, Database, Payload, Stage};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] shaders::Error),
    #[error("SDB2 stage bytecode at {at:#x}: {why}")]
    Stage { at: usize, why: String },
}
pub type Result<T> = std::result::Result<T, Error>;

/// The bytes of an effect's header, before its programs.
const EFFECT_HEADER: usize = 48;

/// Read a complete uncompressed SDB2 v39 database ([`shaders::read`]) and frame its stage programs: each must be one
/// complete SM3 program (through its END token) of the stage the record states. Programs are framed, not lifted; an
/// operation outside a lifter's subset remains bytecode.
pub fn read(bytes: &[u8]) -> Result<Database> {
    let database = shaders::read(bytes)?;
    for record in &database.records {
        let at = usize::try_from(record.offset).unwrap_or(usize::MAX);
        match &record.payload {
            Payload::Missing => {}
            Payload::Program { stage, bytes } => frame(bytes, at, *stage)?,
            Payload::Effect(e) => {
                let vertex_at = at.saturating_add(EFFECT_HEADER);
                if let Some(v) = &e.vertex {
                    frame(v, vertex_at, Stage::Vertex)?;
                }
                if let Some(p) = &e.pixel {
                    let pixel_at = vertex_at.saturating_add(e.vertex.as_ref().map_or(0, Vec::len));
                    frame(p, pixel_at, Stage::Pixel)?;
                }
            }
        }
    }
    Ok(database)
}

/// `bytes`, at file offset `at`, as exactly one complete SM3 program of stage `expected`.
fn frame(bytes: &[u8], at: usize, expected: Stage) -> Result<()> {
    let expected = match expected {
        Stage::Vertex => iridia_interface::Stage::Vertex,
        Stage::Pixel => iridia_interface::Stage::Pixel,
    };
    let (stage, program) = iridia_d3d9::frame(bytes).map_err(|e| Error::Stage {
        at,
        why: e.to_string(),
    })?;
    if stage != expected || program.len() != bytes.len() {
        return Err(Error::Stage {
            at,
            why: format!(
                "{stage:?}, {} of {} bytes; expected complete {expected:?}",
                program.len(),
                bytes.len()
            ),
        });
    }
    Ok(())
}

/// Attach a chosen database as `shaunwhite.shader_library`, replacing a prior one. A failure leaves the world
/// unchanged. The caller chooses the quality file; no path, quality policy, binding or permutation is inferred.
pub fn read_into(world: &mut primase_core::World, bytes: &[u8]) -> Result<()> {
    let database = read(bytes)?;
    world.insert_resource(crate::components::ShaderLibrary(std::sync::Arc::new(database)));
    Ok(())
}
