//! GTA V read straight from an install: archives (RPF7 with their NG or AES ciphers) and resources (RSC7).
//!
//! Arranged by role: [`read`] (the archives, the mount data and a region of the map into the world), [`rage`] (the
//! resource formats, decoded from bytes), [`components`] (the `gta.*` dialect) and [`passes`].
//!
//! Key material is never part of this crate: the caller supplies it ([`Keys`](read::archive::keys::Keys)) and the
//! crate checks it against published digests before using it.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod rage;
pub mod read;

// paths consumers use from before the crate was arranged by role
pub use passes::{collision, lights, lower, pose, rect, select, surfaces, terrain, tint, water, winding};
pub use read::convert;
/// `pipeline::LeaveOutInteriors`, as consumers name it: now [`passes::interiors::LeaveOutInteriors`].
pub mod pipeline {
    pub use crate::passes::interiors::LeaveOutInteriors;
}
/// `region::hour_axis`, as consumers name it: now [`components::state::hour_axis`].
pub mod region {
    pub use crate::components::state::hour_axis;
}
/// `ipl::ipl_base`, as consumers name it: now [`components::state::ipl_base`].
pub mod ipl {
    pub use crate::components::state::ipl_base;
}

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

/// A read past the bytes there are ([`primase_bytes::View`], [`primase_bytes::Cursor`]): the format's error at its
/// offset.
impl From<primase_bytes::BoundsError> for Error {
    fn from(e: primase_bytes::BoundsError) -> Self {
        Error::from(primase_bytes::FieldError {
            what: "bytes",
            bounds: e,
        })
    }
}

/// A read past the bytes there are, of a named format (`c.u32(LE).field("RBF")`).
impl From<primase_bytes::FieldError> for Error {
    fn from(e: primase_bytes::FieldError) -> Self {
        let offset = u64::try_from(e.bounds.absolute()).unwrap_or(u64::MAX);
        let detail = match e.bounds {
            primase_bytes::BoundsError::OutOfBounds { length, total, .. } => {
                format!("{length} bytes past the {total} there are")
            }
            primase_bytes::BoundsError::LengthOverflow { count, stride, .. } => {
                format!("{count} entries of {stride} bytes overflow")
            }
        };
        invalid(e.what, offset, detail)
    }
}

pub(crate) fn invalid(format: &'static str, offset: u64, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}

/// Registers the GTA passes by their declarations: the lowerings by GTA's own rules take no option; the region, the
/// stand-in levels, the collision detail and flagged volumes, the water's clip, the shader technique, day or night and
/// the lights' hour and calibration are the caller's choices, required.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    use passes::*;
    for spec in [
        &roles::CLASSIFY_GTA_TEXTURE_ROLES,
        &pose::GTA_POSE_FRAGMENTS,
        &skin::LOWER_GTA_SKIN,
        &clips::LOWER_GTA_CLIPS,
        &lower::LOWER_GTA_PBR,
        &winding::GTA_WINDING,
        &tint::GTA_TINT,
        &terrain::GTA_TERRAIN_LAYERS,
        &vertex_colours::GTA_VERTEX_COLOURS,
        &select::GTA_SELECT_REGION,
        &collision::LOWER_GTA_COLLISION,
        &select::GTA_SELECT_STAND_INS,
        &collision::GTA_SELECT_COLLISION,
        &collision::LOWER_GTA_EMBEDDED_COLLISION,
        &surfaces::LOWER_GTA_SURFACES,
        &water::GTA_SELECT_WATER,
        &water::LOWER_GTA_WATER,
        &resolve_shaders::RESOLVE_GTA_SHADERS,
        &lights::LOWER_GTA_LIGHTS,
        &interiors::GTA_LEAVE_OUT_INTERIORS,
    ] {
        reg.add(spec);
    }
}

#[cfg(test)]
mod tests {
    /// GTA registers exactly these passes, and each is made under its own name: with no options, but for the
    /// collision lowerings, which require their `foliage` and `river` choices, the region rules their region and the
    /// stand-in selection its levels.
    #[test]
    fn the_gta_passes_register_by_name() {
        let mut reg = primase_core::pass::PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "classify-gta-texture-roles",
            "gta-leave-out-interiors",
            "gta-pose-fragments",
            "gta-select-collision",
            "gta-select-region",
            "gta-select-stand-ins",
            "gta-select-water",
            "gta-terrain-layers",
            "gta-tint",
            "gta-vertex-colours",
            "gta-winding",
            "lower-gta-clips",
            "lower-gta-collision",
            "lower-gta-embedded-collision",
            "lower-gta-lights",
            "lower-gta-pbr",
            "lower-gta-skin",
            "lower-gta-surfaces",
            "lower-gta-water",
            "resolve-gta-shaders",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let volumes: primase_core::pass::Options = [("foliage", "keep"), ("river", "leave-out")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        for name in names {
            let region: primase_core::pass::Options = [("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            let required = if name.starts_with("lower-gta-") && name.ends_with("collision") {
                Some(volumes.clone())
            } else if name == "gta-select-region" {
                Some(region)
            } else if name == "resolve-gta-shaders" {
                Some(
                    [("technique", "deferred_draw"), ("unset-textures", "white")]
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect(),
                )
            } else if name == "gta-select-stand-ins" {
                Some(
                    [("levels", "2;3;4")]
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect(),
                )
            } else {
                let water = ("water-colour", "48,98,122,190");
                let required: [(&str, &[(&str, &str)]); 5] = [
                    ("lower-gta-pbr", &[("night", "false"), water]),
                    ("lower-gta-water", &[water]),
                    ("gta-select-water", &[("clip", "region")]),
                    (
                        "lower-gta-lights",
                        &[("hour", "every"), ("candela-per-unit", "1771")],
                    ),
                    ("gta-select-collision", &[("detail", "plain")]),
                ];
                required.into_iter().find(|(n, _)| *n == name).map(|(_, o)| {
                    o.iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect()
                })
            };
            assert_eq!(reg.requires_options(name), required.is_some(), "{name}");
            let options = match required {
                Some(o) => {
                    assert!(
                        reg.make(name, &primase_core::pass::Options::new()).is_err(),
                        "{name}"
                    );
                    o
                }
                None => primase_core::pass::Options::new(),
            };
            let pass = reg.make(name, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(pass.name(), name);
        }
    }

    /// The selection passes that leave entities out declare it, so the pass manager accepts a name that only the
    /// entities they despawned held disappearing.
    #[test]
    fn passes_that_leave_entities_out_declare_it() {
        let mut reg = primase_core::pass::PassRegistry::new();
        super::register(&mut reg);
        let p = reg
            .parse(
                "gta-select-region{x0=0,y0=0,x1=1,y1=1}, gta-select-stand-ins{levels=2;3;4}, gta-select-collision{detail=plain}, \
                 gta-leave-out-interiors, gta-select-water{clip=region}, lower-gta-collision{foliage=leave-out,river=leave-out}",
            )
            .unwrap();
        for pass in &p.passes {
            assert!(pass.despawns_entities(), "{}", pass.name());
        }
    }

    /// Every GTA pass in the order a conversion runs them (a selection on the read world, then the passes over a
    /// loaded part, by day, at night and with the hour kept as state; examples, not a recipe: the program chooses its
    /// own) is accepted by the pass manager: no pass provides what an earlier one settled (core.surfaces after
    /// lower-gta-surfaces). Declarations only, no game data.
    #[test]
    fn the_gta_passes_in_conversion_order_are_not_refused() {
        use primase_core::pass::{Error, PassManager, PassRegistry, RunReport};
        let mut r = PassRegistry::new();
        r.add(&primase_core::SELECT_LOD);
        r.add(&primase_core::state::RESOLVE_STATE);
        r.add(&primase_image::NORMALIZE_NORMALS);
        crate::register(&mut r);
        let m = PassManager::new(r);
        let rect = "x0=0,y0=0,x1=1,y1=1";
        let leave = "foliage=leave-out,river=leave-out";
        let select = |assign: &str, lights: &str| {
            format!(
                "resolve-state{{{assign}}}, gta-select-region{{{rect},min_size=0.5,max_priority=3}}, \
                gta-leave-out-interiors, lower-gta-lights{{{lights}{rect}}}, gta-select-collision{{detail=plain}}, \
                lower-gta-collision{{{rect},{leave}}}, lower-gta-surfaces, gta-select-water{{{rect},clip=kept}}"
            )
        };
        let water = "water-colour=48,98,122,190";
        let part = |form: &str, night: &str| {
            format!(
                "select-lod{{form={form}}}, lower-gta-embedded-collision{{{leave}}}, lower-gta-surfaces, \
                gta-pose-fragments, lower-gta-pbr{{night={night},{water}}}, gta-winding, gta-tint, \
                gta-terrain-layers, gta-vertex-colours, lower-gta-water{{{water}}}, normalize-normals"
            )
        };
        let pipelines = [
            select(
                "assign=gta.map:online;gta.hour:12",
                "hour=every,candela-per-unit=1771,",
            ),
            part("finest", "false"),
            select(
                "assign=gta.map:online;gta.hour:22",
                "hour=22,candela-per-unit=1771,",
            ),
            part("1", "true"),
            select(
                "assign=gta.map:story,keep=gta.hour",
                "hour=state,candela-per-unit=1771,",
            ),
            part("finest", "state"),
        ];
        for text in pipelines {
            let p = m.registry.parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            // an empty world: the run may stop for other reasons, never this one
            let r = m.run(&mut primase_core::World::new(), &p, &mut RunReport::default());
            assert!(
                !matches!(r, Err(Error::SettledThenChanged { .. })),
                "{text}: {r:?}"
            );
        }
    }
}
