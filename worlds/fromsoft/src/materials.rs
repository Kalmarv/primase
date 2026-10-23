//! FLVER materials resolved with their MTD definitions as source records ([`resolve`]); the records themselves are
//! components ([`Material`]). Complete MTD parameters and resolved slots survive for a later layered bake.
use crate::Game;
pub use crate::components::materials::{
    Binding, Definition, LayeredMaterial, Material, SkipClass, identity_digest,
};
use crate::{
    flver,
    mtd::{MaterialDefinition, Value},
};
use std::collections::BTreeMap;

/// The addressing a FLVER tiling type states; `None` where it states none.
pub fn address(tiling: flver::Tiling) -> Option<primase_core::shader::AddressMode> {
    use primase_core::shader::AddressMode;
    match tiling {
        flver::Tiling::None => None,
        flver::Tiling::Repeat => Some(AddressMode::Repeat),
        flver::Tiling::MirrorRepeat => Some(AddressMode::Mirror),
        flver::Tiling::Clamp => Some(AddressMode::Clamp),
        flver::Tiling::Border => Some(AddressMode::Border),
        flver::Tiling::MirrorOnce => Some(AddressMode::MirrorOnce),
    }
}

pub fn stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_lowercase()
}
fn leaf_stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_owned()
}
/// What a source class rule matches: the shader's name or the material's own (its MTD or `.matxml` stem).
#[derive(Clone, Copy, Debug)]
enum Target {
    Shader,
    Material,
}
/// How a rule matches its target.
#[derive(Clone, Copy, Debug)]
enum Pattern {
    Prefix(&'static str),
    Contains(&'static str),
    Suffix(&'static str),
}
/// The class a rule gives.
#[derive(Clone, Copy, Debug)]
enum Class {
    Skip(SkipClass),
    Water,
    Decal,
    Leaves,
}
/// A game's shader-family names and what the game draws them as: a documented table from its source names to the
/// shared classes, first match wins; a name no row matches is a surface, classed by its blend mode and alpha reference.
type ClassTable = [(Target, Pattern, Class)];

/// Sekiro's families (its shader names and MTD names).
const SEKIRO_CLASSES: &ClassTable = &[
    (
        Target::Shader,
        Pattern::Prefix("M[Sky]"),
        Class::Skip(SkipClass::Sky),
    ),
    (
        Target::Shader,
        Pattern::Contains("CloudFog"),
        Class::Skip(SkipClass::Sky),
    ),
    (
        Target::Shader,
        Pattern::Contains("GroundFog"),
        Class::Skip(SkipClass::Sky),
    ),
    (
        Target::Shader,
        Pattern::Prefix("VolumeTest"),
        Class::Skip(SkipClass::LightVolume),
    ),
    (
        Target::Shader,
        Pattern::Prefix("GXFlver_Shadow"),
        Class::Skip(SkipClass::Shadow),
    ),
    (
        Target::Material,
        Pattern::Prefix("M[Shadow]"),
        Class::Skip(SkipClass::Shadow),
    ),
    (
        Target::Shader,
        Pattern::Contains("Tessellation_Water"),
        Class::Water,
    ),
    (Target::Shader, Pattern::Contains("MeshDecal"), Class::Decal),
    (Target::Material, Pattern::Suffix("_md"), Class::Decal),
    (Target::Shader, Pattern::Prefix("M[Translucency]"), Class::Leaves),
];

/// Elden Ring's families, from its material binaries' shader names: skies and cloud cards (`M[Sky9]`,
/// `M[Sky]_Cloudiness_A`, `M[Clouds5]`, `M[DistantClouds]_001`, `M[Atmosphere][Clouds]_001`), light shafts, shadow
/// proxies (`C[AMSN]_Shadow`), water (`M[Water][river]`, `M[MuddyWater]`, `M[Waterfall2]`), mesh decals
/// (`M[AMSN_V]_MeshDecalBlend`) and translucent foliage (`M[AMSN_V][Ov_N][Translucency]`).
const ELDEN_CLASSES: &ClassTable = &[
    (
        Target::Shader,
        Pattern::Prefix("M[Sky"),
        Class::Skip(SkipClass::Sky),
    ),
    (
        Target::Shader,
        Pattern::Contains("Clouds"),
        Class::Skip(SkipClass::Sky),
    ),
    (
        Target::Shader,
        Pattern::Contains("LightShaft"),
        Class::Skip(SkipClass::LightVolume),
    ),
    (
        Target::Shader,
        Pattern::Suffix("_Shadow"),
        Class::Skip(SkipClass::Shadow),
    ),
    (Target::Shader, Pattern::Contains("Water"), Class::Water),
    (Target::Shader, Pattern::Contains("MeshDecal"), Class::Decal),
    (Target::Shader, Pattern::Contains("[Translucency]"), Class::Leaves),
];

/// The class table of `game`.
fn classes(game: Game) -> &'static ClassTable {
    match game {
        Game::Sekiro => SEKIRO_CLASSES,
        Game::EldenRing => ELDEN_CLASSES,
    }
}

/// A Sekiro FLVER material entry with its MTD `definition`: [`resolve_definition`] for an MTD.
pub fn resolve(
    source: &flver::Material,
    definition: Option<&MaterialDefinition>,
    exists: impl Fn(&str) -> bool,
) -> Material {
    resolve_definition(
        source,
        definition.cloned().map(Definition::Mtd).as_ref(),
        Game::Sekiro,
        exists,
    )
}

/// A FLVER material entry with its `definition` (an MTD or a material binary) as the source states it: its bindings
/// (each with whether `exists` finds its texture, the caller's texture catalog), its shader, the class its shader
/// family states in `game`'s table (water, decal, leaves, alpha by blend mode, cutout by alpha reference) or the skip
/// class of one the game draws as something other than a surface, its alpha reference and, for Sekiro's layered
/// shaders, its layered definition. No texture is picked here: the preview a lowering draws is [`crate::preview`]'s.
pub fn resolve_definition(
    source: &flver::Material,
    definition: Option<&Definition>,
    game: Game,
    exists: impl Fn(&str) -> bool,
) -> Material {
    let mut bindings = BTreeMap::new();
    if let Some(d) = definition {
        for (kind, path, uv_number) in d.textures() {
            if !path.trim().is_empty() {
                bindings.insert(
                    kind.to_owned(),
                    Binding {
                        available: exists(&stem(path)),
                        texture: stem(path),
                        scale: [1.; 2],
                        uv_number,
                        address: [None; 2],
                    },
                );
            }
        }
    }
    for t in &source.textures {
        if !t.path.trim().is_empty() {
            let uv_number = definition
                .and_then(|d| d.textures().into_iter().find(|x| x.0 == t.kind))
                .map_or(1, |t| t.2);
            bindings.insert(
                t.kind.clone(),
                Binding {
                    available: exists(&stem(&t.path)),
                    texture: stem(&t.path),
                    scale: t.scale,
                    uv_number,
                    address: t.tiling.map(address),
                },
            );
        }
    }
    let parameter = |name: &str| definition.and_then(|d| d.int(name)).unwrap_or(0);
    let blend = parameter("g_BlendMode");
    let aref = parameter("g_AlphaRef");
    let mut m = Material {
        mtd: leaf_stem(&source.mtd),
        definition: definition.cloned(),
        gx: source.gx.clone(),
        shader: definition.map_or(String::new(), |d| leaf_stem(d.shader())),
        class: "opaque".into(),
        skip: None,
        cutoff: 0.5,
        bindings,
        layered: None,
    };
    let class = classes(game).iter().find_map(|(target, pattern, class)| {
        let name = match target {
            Target::Shader => m.shader.as_str(),
            Target::Material => m.mtd.as_str(),
        };
        let found = match pattern {
            Pattern::Prefix(p) => name.starts_with(p),
            Pattern::Contains(p) => name.contains(p),
            Pattern::Suffix(p) => name.ends_with(p),
        };
        found.then_some(*class)
    });
    match class {
        Some(Class::Skip(skip @ (SkipClass::Sky | SkipClass::LightVolume))) => m.skip = Some(skip),
        // an additive blend draws a light volume whatever else the family's name says
        _ if blend == 4 => m.skip = Some(SkipClass::LightVolume),
        Some(Class::Skip(skip)) => m.skip = Some(skip),
        Some(Class::Water) => m.class = "water".into(),
        Some(Class::Decal) => m.class = "decal".into(),
        Some(Class::Leaves) => {
            m.class = "leaves".into();
            if aref > 0 {
                m.cutoff = aref as f32 / 255.;
            }
        }
        None if blend == 2 => m.class = "alpha".into(),
        None if aref > 0 => {
            m.class = "cutout".into();
            m.cutoff = aref as f32 / 255.;
        }
        None => {}
    }
    if let Some(d) = definition.and_then(Definition::mtd)
        && m.shader.starts_with("M[Multiple")
        && m.skip.is_none()
        && d.parameters.iter().any(|p| {
            p.name == "GXFT_SATStaticSwitch"
                && matches!(&p.value,Value::Int(v) if v.first().is_some_and(|x|*x>=0))
        })
    {
        m.layered = Some(LayeredMaterial {
            mtd: m.mtd.clone(),
            definition: d.clone(),
            bindings: m.bindings.clone(),
        });
    }
    m
}
