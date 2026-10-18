//! GTA V read straight from an install: archives (RPF7 with their NG or AES ciphers) and resources (RSC7).
//!
//! Key material is never part of this crate: the caller supplies it ([`Keys`]) and the crate checks it against
//! published digests before using it.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod archive_set;
pub mod assets;
pub mod bounds;
pub mod collision;
pub mod components;
pub mod convert;
pub mod drawable;
pub mod gtxd;
pub mod ipl;
mod key_hashes;
pub mod keys;
mod le;
pub mod lights;
pub mod lower;
pub mod map;
pub mod meta;
pub mod mount;
pub mod ng;
pub mod pipeline;
pub mod pose;
mod rbf;
pub mod region;
pub mod resource;
pub mod rpf;
pub mod select;
pub mod sga;
pub mod shader_class;
pub mod shader_names;
pub mod surfaces;
pub mod terrain;
pub mod texture;
pub mod tint;
pub mod water;
pub mod winding;
mod xml;

pub use archive_set::{ArchiveSet, FileRef, Install, MapState};
pub use keys::Keys;
pub use resource::{PageFlags, Resource};
pub use rpf::{Archive, Encryption, Entry, EntryKind};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: u64,
        detail: String,
    },
    #[error("key material: {0}")]
    Keys(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("compression: {0}")]
    Compression(#[from] primase_compress::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(format: &'static str, offset: u64, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}

/// Registers the GTA passes by name: `gta-pose-fragments`, `lower-gta-pbr{night}`, `gta-winding`, `gta-tint`, `gta-terrain-layers`,
/// `gta-vertex-colours`, `gta-select-region{x0,y0,x1,y1,max_priority,min_size}`, `gta-select-stand-ins{levels,x0,y0,x1,y1}`,
/// `gta-leave-out-interiors`,
/// `lower-gta-lights{hour,x0,y0,x1,y1}`, `gta-select-collision{detail}`, `lower-gta-collision{x0,y0,x1,y1}`, `lower-gta-embedded-collision`,
/// `lower-gta-surfaces`, `gta-select-water{x0,y0,x1,y1}` and `lower-gta-water`.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    use primase_core::pass::{Options, Pass};
    fn none(o: &Options, pass: impl Pass + 'static) -> std::result::Result<Box<dyn Pass>, String> {
        match o.keys().next() {
            Some(k) => Err(format!("unknown option {k}")),
            None => Ok(Box::new(pass)),
        }
    }
    reg.register("gta-pose-fragments", |o| none(o, pose::PoseFragments));
    reg.register("lower-gta-pbr", |o| {
        Ok(Box::new(lower::LowerGtaPbr::from_options(o)?))
    });
    reg.register("gta-winding", |o| none(o, winding::GtaWinding));
    reg.register("gta-tint", |o| none(o, tint::GtaTintVariants));
    reg.register("gta-terrain-layers", |o| none(o, terrain::GtaTerrainLayers));
    reg.register("gta-vertex-colours", |o| none(o, lower::GtaVertexColours));
    reg.register("gta-select-region", |o| {
        Ok(Box::new(select::SelectRegion::from_options(o)?))
    });
    reg.register("lower-gta-collision", |o| {
        Ok(Box::new(collision::LowerGtaCollision::from_options(o)?))
    });
    reg.register("gta-select-stand-ins", |o| {
        Ok(Box::new(select::SelectStandIns::from_options(o)?))
    });
    reg.register("gta-select-collision", |o| {
        Ok(Box::new(collision::SelectCollision::from_options(o)?))
    });
    reg.register("lower-gta-embedded-collision", |o| {
        none(o, collision::LowerGtaEmbeddedCollision)
    });
    reg.register("lower-gta-surfaces", |o| none(o, surfaces::LowerGtaSurfaces));
    reg.register("gta-select-water", |o| {
        Ok(Box::new(water::SelectGtaWater::from_options(o)?))
    });
    reg.register("lower-gta-water", |o| none(o, water::LowerGtaWater));
    reg.register("lower-gta-lights", |o| {
        Ok(Box::new(lights::LowerGtaLights::from_options(o)?))
    });
    reg.register("gta-leave-out-interiors", |o| {
        Ok(Box::new(pipeline::LeaveOutInteriors::from_options(o)?))
    });
}

#[cfg(test)]
mod tests {
    /// Every pass registers: the registry makes each with no options, and a pass that refuses them panics there.
    #[test]
    fn every_pass_registers() {
        let mut reg = primase_core::pass::PassRegistry::new();
        super::register(&mut reg);
    }

    /// The selection passes that leave entities out declare it, so the pass manager accepts a name that only the
    /// entities they despawned held disappearing.
    #[test]
    fn passes_that_leave_entities_out_declare_it() {
        let mut reg = primase_core::pass::PassRegistry::new();
        super::register(&mut reg);
        let p = reg
            .parse("gta-select-region, gta-select-stand-ins, gta-select-collision, gta-leave-out-interiors")
            .unwrap();
        for pass in &p.passes {
            assert!(pass.despawns_entities(), "{}", pass.name());
        }
    }
}
