//! primase core: the world store, the core components, the frame, the pass manager and gates.
//!
//! See `docs/DESIGN.md` §3-4 and CONTRIBUTING.md for the conventions every crate follows.
//! Game-independent animation descriptions (events, draw groups, material parameters and
//! controllers) are data only. Readers retain source facts; programs select lowering passes,
//! and toolkit evaluators supply time-dependent behavior. JSON contracts are in `docs/FORMAT.md`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod asset;
mod atmosphere;
pub use atmosphere::{Atmosphere, Exposure, Fog, FogFalloff, VolumetricFog};
pub mod clip;
mod clip_identity;
pub use clip_identity::{ClipKey, ClipLength};
mod clip_events;
pub use clip_events::{ClipEvent, ClipEvents, EventValue, EventValueData};
mod content;
pub use content::{ContentNote, DisplayName, NameOrigin, Record};
mod controller;
pub use controller::{
    Controller, ControllerBinding, ControllerLayer, ControllerParameter, ControllerParameterKind,
    ControllerState, LayerBlending, Motion,
};
mod draw_groups;
pub use draw_groups::{DrawGroupDefaults, DrawGroups};
mod material_parameters;
pub use material_parameters::MaterialParameters;
mod primitive_keys;
pub use primitive_keys::PrimitiveKeys;
pub mod components;
pub mod convert;
pub mod detail;
mod environment;
pub use environment::{Ambient, AmbientLight, Environment};
pub mod frame;
pub mod json;
pub mod layered;
pub mod ledger;
mod lod;
pub mod names;
pub use lod::{LodChoice, LodForm, Lods, SELECT_LOD, SelectLod};
mod marker;
pub use marker::{Curve, Marker, kind_is_valid};
mod spawn;
pub use spawn::{PLACE_SPAWN, PlaceSpawn, SpawnAt, centre_ground};
mod collider;
pub use collider::{Collider, Diagonal, Heightfield, Shape};
mod draw_order;
pub use draw_order::{DrawLayer, DrawOrder};
mod material_usage;
pub use material_usage::MaterialUsage;
mod vertex_color;
pub use vertex_color::{VertexColor, color0_drawn, uses_color0};
pub mod morphs;
pub mod parallel;
pub mod pass;
mod pass_options;
pub mod pbr;
pub mod print;
pub mod provenance;
pub mod render_graph;
pub mod rig;
pub mod shader;
pub mod skeleton;
pub mod skin;
pub use asset::{
    Asset, AssetField, PARTITION_BY_ASSET, PLACE_ASSETS, PartitionByAsset, PlaceAssets, Role, SELECT_ASSETS,
    SelectAssets,
};
pub use clip::{
    AdditiveReference, Channel, Clip, ClipBinding, ClipTarget, Extrapolation, Interpolation, Property,
    RootMotion, RootSpace,
};
pub use morphs::{Deltas, Frame as MorphFrame, MorphId, Morphs, Target as MorphTarget};
pub use provenance::{Origin, Provenance};
pub use rig::{
    Attachment, JointMap, JointOffset, JointPlacement, MorphWeights, RigInstance, RigPose, SkinInstance,
    Socket, SocketId, Sockets,
};
pub use skeleton::{Joint, JointId, Rest, Skeleton};
pub use skin::{Binding, BindingId, Deformation, Influences, Skin, Slot, WeightRule};
mod priority;
pub use priority::{DROP_OPTIONAL, DropOptional, Priority};
pub mod state;
pub mod summary;
pub mod texformat;
mod texture_layers;
pub use texture_layers::{TextureLayers, TextureShape};
pub mod translate;
pub mod vector;
pub mod world;

pub use components::*;
pub use detail::DetailedMaterial;
pub use json::JsonError;
pub use layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
pub use texformat::TexFormat;
pub use world::{
    Column, ColumnMut, Component, ComponentJson, ComponentJsonError, Entity, Geometry, IdError, Linearity,
    NameClash, Opaque, OpaqueData, Resource, VertexBlend, World, remap_list,
};

/// Register core's generic passes by their declarations, as every crate registers its own: a program builds its
/// registry from these. What each takes: [`pass::PassRegistry::describe`], or the declaration ([`SELECT_LOD`],
/// [`DROP_OPTIONAL`], [`PLACE_SPAWN`], [`PLACE_ASSETS`], [`SELECT_ASSETS`], [`PARTITION_BY_ASSET`],
/// [`state::RESOLVE_STATE`], [`state::SELECT_OUTFIT`], [`shader::SELECT_VARIANT`],
/// [`translate::TRANSLATE_WORLD`]).
pub fn register(reg: &mut pass::PassRegistry) {
    for spec in [
        &SELECT_LOD,
        &DROP_OPTIONAL,
        &PLACE_SPAWN,
        &PLACE_ASSETS,
        &SELECT_ASSETS,
        &PARTITION_BY_ASSET,
        &state::RESOLVE_STATE,
        &state::SELECT_OUTFIT,
        &shader::SELECT_VARIANT,
        &translate::TRANSLATE_WORLD,
    ] {
        reg.add(spec);
    }
}

#[cfg(test)]
mod tests {
    /// Core registers exactly these passes by their declarations: the ones with a required option or group are made
    /// only with their options, every one says what it takes, and a pipeline without a required option is refused in
    /// the declaration's words.
    #[test]
    fn the_core_passes_register_by_name() {
        let mut reg = crate::pass::PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "drop-optional",
            "partition-by-asset",
            "place-assets",
            "place-spawn",
            "resolve-state",
            "select-assets",
            "select-lod",
            "select-outfit",
            "select-variant",
            "translate-world",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let requiring = [
            "place-spawn",
            "select-assets",
            "select-lod",
            "select-outfit",
            "select-variant",
            "translate-world",
        ];
        for name in names {
            assert_eq!(reg.requires_options(name), requiring.contains(&name), "{name}");
            let described = reg
                .describe(name)
                .unwrap_or_else(|| panic!("{name} has no declaration"));
            assert!(described.starts_with(name), "{described}");
            let bare = reg.make(name, &crate::pass::Options::new());
            assert_eq!(bare.is_err(), requiring.contains(&name), "{name}");
        }
        let refused = |name: &str| {
            reg.make(name, &crate::pass::Options::new())
                .err()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            refused("translate-world"),
            "pass translate-world: by is required: X,Y,Z, metres in core's frame: the offset is the program's choice"
        );
        assert!(
            refused("select-assets").starts_with("pass select-assets: one of keys or names is required (")
        );
        assert!(refused("select-lod").starts_with("pass select-lod: form is required: "));
        assert!(refused("place-spawn").starts_with("pass place-spawn: one of at or east is required ("));
        assert!(
            reg.describe("select-assets")
                .unwrap()
                .starts_with("select-assets{(keys|names)*}\n")
        );
    }
}
