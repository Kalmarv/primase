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

pub(crate) fn invalid(format: &'static str, offset: u64, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}

/// Registers the GTA passes by name: `gta-pose-fragments`, `lower-gta-skin`, `lower-gta-clips`, `lower-gta-pbr{night}` (`night` required), `gta-winding`, `gta-tint`, `gta-terrain-layers`,
/// `gta-vertex-colours`, `gta-select-region{x0,y0,x1,y1,max_priority,min_size}`, `gta-select-stand-ins{levels,x0,y0,x1,y1}`,
/// `gta-leave-out-interiors`,
/// `lower-gta-lights{hour,candela-per-unit,x0,y0,x1,y1}` (`hour` and `candela-per-unit` required), `gta-select-collision{detail}` (`detail` required), `lower-gta-collision{x0,y0,x1,y1,foliage,river}`,
/// `lower-gta-embedded-collision{foliage,river}` (their `foliage` and `river` required),
/// `lower-gta-surfaces`, `gta-select-water{x0,y0,x1,y1,clip}` (`clip` required), `lower-gta-water` and `resolve-gta-shaders{technique,
/// unset-textures}` (both required).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    use primase_core::pass::{Options, Pass};
    fn none(o: &Options, pass: impl Pass + 'static) -> std::result::Result<Box<dyn Pass>, String> {
        match o.keys().next() {
            Some(k) => Err(format!("unknown option {k}")),
            None => Ok(Box::new(pass)),
        }
    }
    reg.register("classify-gta-texture-roles", |o| {
        none(o, passes::roles::ClassifyGtaTextureRoles)
    });
    reg.register("gta-pose-fragments", |o| none(o, passes::pose::PoseFragments));
    reg.register("lower-gta-skin", |o| none(o, passes::skin::LowerGtaSkin));
    reg.register("lower-gta-clips", |o| none(o, passes::clips::LowerGtaClips));
    // day or night lighting is the caller's choice: no default
    reg.register_requiring(
        "lower-gta-pbr",
        |o| Ok(Box::new(passes::lower::LowerGtaPbr::from_options(o)?)),
        &[("night", "false")],
    );
    reg.register("gta-winding", |o| none(o, passes::winding::GtaWinding));
    reg.register("gta-tint", |o| none(o, passes::tint::GtaTintVariants));
    reg.register("gta-terrain-layers", |o| {
        none(o, passes::terrain::GtaTerrainLayers)
    });
    reg.register("gta-vertex-colours", |o| {
        none(o, passes::vertex_colours::GtaVertexColours)
    });
    // the region is the caller's choice: no default
    reg.register_requiring(
        "gta-select-region",
        |o| Ok(Box::new(passes::select::SelectRegion::from_options(o)?)),
        &[("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1")],
    );
    // whether flagged collision is dropped is the caller's choice: no default
    let volumes = [("foliage", "leave-out"), ("river", "leave-out")];
    reg.register_requiring(
        "lower-gta-collision",
        |o| Ok(Box::new(passes::collision::LowerGtaCollision::from_options(o)?)),
        &volumes,
    );
    // the levels a far view draws are the caller's choice: no default
    reg.register_requiring(
        "gta-select-stand-ins",
        |o| Ok(Box::new(passes::select::SelectStandIns::from_options(o)?)),
        &[("levels", "2;3;4")],
    );
    // which detail of the map's collision is kept is the caller's choice: no default
    reg.register_requiring(
        "gta-select-collision",
        |o| Ok(Box::new(passes::collision::SelectCollision::from_options(o)?)),
        &[("detail", "plain")],
    );
    reg.register_requiring(
        "lower-gta-embedded-collision",
        |o| {
            Ok(Box::new(
                passes::collision::LowerGtaEmbeddedCollision::from_options(o)?,
            ))
        },
        &volumes,
    );
    reg.register("lower-gta-surfaces", |o| {
        none(o, passes::surfaces::LowerGtaSurfaces)
    });
    // how the water is clipped is the caller's choice: no default
    reg.register_requiring(
        "gta-select-water",
        |o| Ok(Box::new(passes::water::SelectGtaWater::from_options(o)?)),
        &[("clip", "region")],
    );
    reg.register("lower-gta-water", |o| none(o, passes::water::LowerGtaWater));
    // which technique of an effect a draw uses (pass, vertex format, alpha mode) is the draw's choice: no default
    reg.register_requiring(
        "resolve-gta-shaders",
        |o| {
            Ok(Box::new(
                passes::resolve_shaders::ResolveGtaShaders::from_options(o)?,
            ))
        },
        &[("technique", "deferred_draw"), ("unset-textures", "white")],
    );
    // which hour's lights are drawn, and their calibration, are the caller's choices: no defaults
    reg.register_requiring(
        "lower-gta-lights",
        |o| Ok(Box::new(passes::lights::LowerGtaLights::from_options(o)?)),
        &[("hour", "every"), ("candela-per-unit", "1771")],
    );
    reg.register("gta-leave-out-interiors", |o| {
        Ok(Box::new(passes::interiors::LeaveOutInteriors::from_options(o)?))
    });
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
                let required: [(&str, &[(&str, &str)]); 4] = [
                    ("lower-gta-pbr", &[("night", "false")]),
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
        r.register("normalize-normals", |_| {
            Ok(Box::new(primase_image::NormalizeNormals))
        });
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
        let part = |form: &str, night: &str| {
            format!(
                "select-lod{{form={form}}}, lower-gta-embedded-collision{{{leave}}}, lower-gta-surfaces, \
                gta-pose-fragments, lower-gta-pbr{{night={night}}}, gta-winding, gta-tint, gta-terrain-layers, \
                gta-vertex-colours, lower-gta-water, normalize-normals"
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
