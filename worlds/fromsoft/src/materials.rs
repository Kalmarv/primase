//! FLVER materials resolved with their MTD definitions as source records ([`resolve`]); the records themselves are
//! components ([`Material`]). Complete MTD parameters and resolved slots survive for a later layered bake.
use crate::Game;
pub use crate::components::materials::{
    Binding, Definition, DefinitionRef, LayeredMaterial, Material, MaterialView, ParameterValue, SkipClass,
    identity_digest,
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
    crate::names::fold(leaf.rsplit_once('.').map_or(leaf, |x| x.0))
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
    // water by the family's name, as Elden Ring's table has it: the tessellated sea and ponds
    // (`M[Interaction]_Tessellation_Water`), and the rivers and waterfalls whose shaders the scroll policy names
    // (`M[ARSN]_Water_3normaltet` on m25_00's `川` and `滝`, `M[Flow]_Water`)
    (Target::Shader, Pattern::Contains("Water"), Class::Water),
    (Target::Shader, Pattern::Contains("MeshDecal"), Class::Decal),
    (Target::Material, Pattern::Suffix("_md"), Class::Decal),
    (Target::Shader, Pattern::Prefix("M[Translucency]"), Class::Leaves),
];

/// Elden Ring's families, from its material binaries' shader names: skies and cloud cards (`M[Sky9]`,
/// `M[Sky]_Cloudiness_A`, `M[Clouds5]`, `M[DistantClouds]_001`, `M[Atmosphere][Clouds]_001`), light shafts, shadow
/// proxies (`C[AMSN]_Shadow`), SpeedTree's billboard cards (`SpeedTree_billboard`), water (`M[Water][river]`,
/// `M[MuddyWater]`, `M[Waterfall2]`), mesh decals (`M[AMSN_V]_MeshDecalBlend`) and translucent foliage
/// (`M[AMSN_V][Ov_N][Translucency]`).
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
    (
        Target::Shader,
        Pattern::Prefix("SpeedTree_billboard"),
        Class::Skip(SkipClass::Billboard),
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
        // Sekiro: an additive blend draws a light volume whatever else the family's name says
        _ if blend == 4 && game == Game::Sekiro => m.skip = Some(SkipClass::LightVolume),
        Some(Class::Skip(skip)) => m.skip = Some(skip),
        // Elden Ring: an additive blend is a surface drawn over what is behind it (a character's emissive overlay: c2200's
        // `C[c2200]_Body` and `_Wing`, with an albedo of their own), not a volume; the light volumes are the shaders
        // its table names
        _ if blend == 4 => m.class = "additive".into(),
        Some(Class::Water) => m.class = "water".into(),
        Some(Class::Decal) => m.class = "decal".into(),
        Some(Class::Leaves) => {
            m.class = "leaves".into();
            if let Some(reference) = alpha_reference(aref) {
                m.cutoff = reference;
            }
        }
        None if blend == 2 => m.class = "alpha".into(),
        None if aref > 0 => {
            m.class = "cutout".into();
            m.cutoff = alpha_reference(aref).unwrap_or(m.cutoff);
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

/// The alpha test's reference a material states, from the first word of its `g_AlphaRef` (`raw`): the word over 255
/// where it is above 0, none where it is 0 or below (the game's plain program, which tests nothing). The one rule the
/// material's class, its binding's [`primase_core::shader::ConstantRole::AlphaReference`] and the bakes take it by.
pub fn alpha_reference(raw: i32) -> Option<f32> {
    (raw > 0).then(|| raw as f32 / 255.)
}

/// The two words of the alpha test's constant as the engine fills them from the material's `g_AlphaRef`: each over 255, the
/// first -100 (never discards) where the material states none above 0, the second 0 where it states none. The values the
/// layer bind freezes ([`crate::passes::layer_bake::resolve::resolve`] states `FC_AlphaRef.xy` by the same rule).
pub fn engine_alpha_reference(material: &Material) -> [f32; 2] {
    let word = |n: usize| match material.view()?.definition.parameter("g_AlphaRef")? {
        ParameterValue::Int(v) => v.get(n).copied().and_then(alpha_reference),
        _ => None,
    };
    [word(0).unwrap_or(-100.), word(1).unwrap_or(0.)]
}

/// What a lowering counts where a material's alpha cut is not its bound program's own ([`alpha_cut`]).
pub const UNBOUND_ALPHA_CUT: &str = "alpha cut from the material's g_AlphaRef: its program is not bound";

/// What a lowering counts where a material states an alpha reference its bound program's discard does not compare
/// against ([`alpha_cut`]): the program decides, and the surface is not cut.
pub const UNTESTED_ALPHA_REFERENCE: &str = "material states an alpha reference its program does not test";

/// A portable mask cannot reproduce the stochastic pre-transform comparison.
pub const DITHERED_ALPHA_CUT: &str = "dithered alpha test lowered as a hard cutoff";
/// The tested scalar reaches alpha through arithmetic, so cutting the output is not the original test.
pub const TRANSFORMED_ALPHA_CUT: &str = "pre-transform alpha test lowered as a hard cutoff";
/// What a lowering counts where the binding states no reference because the program's discard compares a value it never
/// writes (an Elden Ring G-buffer program writes no target's alpha: `C[Fur]` discards where its strand card's alpha
/// times the vertex colour's is below `FC_AlphaRef.x`), and the lowering read the tested slot from the program's
/// dataflow and made the base colour of it ([`crate::passes::program_reads::Reads::coverage`]): the base colour's
/// alpha is the tested value, cut at the material's `g_AlphaRef`.
pub const COVERAGE_ALPHA_CUT: &str = "alpha test of an unwritten value lowered as a hard cutoff of its coverage slot at the material's g_AlphaRef";

/// What a lowering counts where the binding states the program's alpha test as a comparison of a value the program
/// writes nowhere ([`primase_core::shader::AlphaComparison::Unwritten`]) and the lowering could not evaluate that
/// expression with the values the binding states (it names why in its own notice): no cut is made, because none would be
/// the program's.
pub const UNSOLVED_ALPHA_TEST: &str = "alpha test of an unwritten value not evaluated (the expression is not one varying texture alpha with stated constants): not cut";

/// How far a lowering read what a program's alpha test compares, where the binding states no reference for it or
/// states one for a value the program writes nowhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaTestRead {
    /// Not read, or the program has no alpha test: the material's reference is one its program does not test.
    Unread,
    /// The tested slot was read from the program's dataflow and the surface is coloured with it, the rest of the test
    /// (a vertex colour's alpha it is multiplied by) left out ([`COVERAGE_ALPHA_CUT`]).
    Coverage,
    /// The program's own expression for the test was evaluated with the values its binding states, to one varying
    /// texture alpha, and the surface is coloured with that texture: the exact cutoff of its alpha, `None` where the test
    /// discards nothing. Not counted where `dithered` is false: nothing is approximated. `dithered`: the expression holds
    /// the engine's dither noise with a nonzero coefficient, so the cut at the noise mean replaces a stochastic test
    /// ([`DITHERED_ALPHA_CUT`], counted).
    Solved { cut: Option<f32>, dithered: bool },
}

/// [`alpha_cut_read`] where the tested slot was read (`coverage_read`) or not.
pub fn alpha_cut(
    binding: Option<&primase_core::shader::ShaderBinding>,
    material: &Material,
    coverage_read: bool,
) -> (Option<f32>, Option<&'static str>) {
    alpha_cut_read(
        binding,
        material,
        if coverage_read {
            AlphaTestRead::Coverage
        } else {
            AlphaTestRead::Unread
        },
    )
}

/// A material's alpha cut: the cutoff its mask takes, none where it is not cut, and the count a lowering takes where
/// the cut is not the bound program's own.
///
/// With a binding (`resolve-fromsoft-shaders` run), the cut is the pixel stage's
/// [`primase_core::shader::ConstantRole::AlphaReference`]: a reference above 0 its cutoff, none at 0 or below or where
/// the binding states no reference (its program's discard compares against none), the last counted
/// ([`UNTESTED_ALPHA_REFERENCE`]) where the material states one; but where the lowering read the slot the discard tests
/// and colours the surface with it ([`AlphaTestRead::Coverage`]), the material's reference, counted
/// ([`COVERAGE_ALPHA_CUT`]). Where the binding states a test of a value the program writes nowhere
/// ([`primase_core::shader::AlphaComparison::Unwritten`]) the cut is the program's own expression solved
/// ([`AlphaTestRead::Solved`]), uncounted, or none, counted ([`UNSOLVED_ALPHA_TEST`]).
/// Without, the material's class: `cutout` and `leaves` at its cutoff, counted ([`UNBOUND_ALPHA_CUT`]); a solved test
/// is the cut either way.
pub fn alpha_cut_read(
    binding: Option<&primase_core::shader::ShaderBinding>,
    material: &Material,
    read: AlphaTestRead,
) -> (Option<f32>, Option<&'static str>) {
    use primase_core::shader::{AlphaComparison, ConstantRole, Stage};
    match binding {
        Some(b) => {
            let stated = b.stage(Stage::Pixel).and_then(|s| {
                s.constants.iter().find_map(|c| match c.role {
                    ConstantRole::AlphaReference { value, compared } => Some((value, compared)),
                    _ => None,
                })
            });
            let reference = material.view().and_then(|v| alpha_reference(v.alpha_reference()));
            let states = reference.is_some();
            match stated {
                // the program's test is of a value it writes nowhere: its own expression, evaluated, or no cut
                Some((r, AlphaComparison::Unwritten)) => match read {
                    AlphaTestRead::Solved { cut, dithered } => (cut, dithered.then_some(DITHERED_ALPHA_CUT)),
                    _ => (None, (r > 0.).then_some(UNSOLVED_ALPHA_TEST)),
                },
                Some((r, compared)) => (
                    (r > 0.).then_some(r),
                    if r > 0. {
                        match compared {
                            AlphaComparison::Output => None,
                            AlphaComparison::Transformed => Some(TRANSFORMED_ALPHA_CUT),
                            AlphaComparison::Dithered { .. } => Some(DITHERED_ALPHA_CUT),
                            AlphaComparison::Unwritten => Some(UNSOLVED_ALPHA_TEST),
                        }
                    } else {
                        None
                    },
                ),
                None if states && read == AlphaTestRead::Coverage => (reference, Some(COVERAGE_ALPHA_CUT)),
                None if states => (None, Some(UNTESTED_ALPHA_REFERENCE)),
                None => (None, None),
            }
        }
        None => match (read, material.class.as_str()) {
            (AlphaTestRead::Solved { cut, dithered }, _) => (cut, dithered.then_some(DITHERED_ALPHA_CUT)),
            (_, "cutout" | "leaves") => (Some(material.cutoff), Some(UNBOUND_ALPHA_CUT)),
            _ => (None, None),
        },
    }
}

/// [`alpha_mode_read`] where the tested slot was read (`coverage_read`) or not.
pub fn alpha_mode(
    material: &Material,
    binding: Option<&primase_core::shader::ShaderBinding>,
    coverage_read: bool,
    log: &mut primase_core::pass::PassLog,
) -> primase_core::pbr::AlphaMode {
    alpha_mode_read(
        material,
        binding,
        if coverage_read {
            AlphaTestRead::Coverage
        } else {
            AlphaTestRead::Unread
        },
        log,
    )
}

/// A material's alpha mode: blended for the blended classes (`alpha`, `additive`, `decal`, `water`), otherwise a mask at
/// its alpha cut ([`alpha_cut_read`], `read` its) or opaque, the cut's count taken in `log`.
pub fn alpha_mode_read(
    material: &Material,
    binding: Option<&primase_core::shader::ShaderBinding>,
    read: AlphaTestRead,
    log: &mut primase_core::pass::PassLog,
) -> primase_core::pbr::AlphaMode {
    use primase_core::pbr::AlphaMode;
    if matches!(material.class.as_str(), "alpha" | "additive" | "decal" | "water") {
        return AlphaMode::Blend;
    }
    let (cut, count) = alpha_cut_read(binding, material, read);
    if let Some(count) = count {
        log.count(count, 1);
    }
    cut.map_or(AlphaMode::Opaque, |cutoff| AlphaMode::Mask { cutoff })
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pbr::AlphaMode;
    use primase_core::shader::{
        ConstantBinding, ConstantRole, RenderState, ResourceId, ShaderBinding, Stage, StageBinding,
    };

    /// A material of `class` stating `g_AlphaRef` as `aref` (none where `None`), cut at `cutoff` by the read-time rule.
    fn material(class: &str, aref: Option<i32>, cutoff: f32) -> Material {
        let parameters = aref
            .map(|a| crate::mtd::Parameter {
                name: "g_AlphaRef".into(),
                value: crate::mtd::Value::Int(vec![a, 0]),
            })
            .into_iter()
            .collect();
        Material {
            mtd: "m".into(),
            definition: Some(Definition::Mtd(MaterialDefinition {
                shader: String::new(),
                description: String::new(),
                parameters,
                textures: Vec::new(),
                block_lengths: Vec::new(),
            })),
            gx: Vec::new(),
            shader: String::new(),
            class: class.into(),
            skip: None,
            cutoff,
            bindings: BTreeMap::new(),
            layered: None,
        }
    }

    /// A binding whose pixel stage states the alpha reference `reference` (none where `None`).
    fn binding(reference: Option<f32>) -> ShaderBinding {
        let shader = primase_core::World::default().spawn();
        ShaderBinding {
            stages: vec![StageBinding {
                stage: Stage::Pixel,
                variant: None,
                shader,
                inputs: Vec::new(),
                constants: reference
                    .map(|r| ConstantBinding {
                        resource: ResourceId(0),
                        offset: 80,
                        size: 4,
                        name: Some("FC_AlphaRef".into()),
                        role: ConstantRole::AlphaReference {
                            value: r,
                            compared: primase_core::shader::AlphaComparison::Output,
                        },
                        lines: None,
                    })
                    .into_iter()
                    .collect(),
                textures: Vec::new(),
                buffers: Vec::new(),
                constant_buffers: Vec::new(),
                outputs: Vec::new(),
            }],
            state: RenderState::default(),
        }
    }

    /// With a binding the cut is its alpha reference's: a reference above 0 the mask's cutoff, 0, a negative one or none
    /// no mask, the last counted where the material states a reference. Without, the read-time class, counted. A
    /// blended class is blended either way.
    #[test]
    fn the_alpha_cut_is_the_bound_programs_reference_and_the_material_class_without_one() {
        let mode_read = |m: &Material, b: Option<&ShaderBinding>, coverage_read: bool| {
            let mut log = primase_core::pass::PassLog::default();
            let mode = alpha_mode(m, b, coverage_read, &mut log);
            (mode, log.counts.keys().cloned().collect::<Vec<_>>())
        };
        let mode = |m: &Material, b: Option<&ShaderBinding>| mode_read(m, b, false);
        let cutout = material("cutout", Some(128), 128. / 255.);
        let reference = alpha_reference(128).unwrap();
        assert_eq!(reference, 128. / 255.);
        assert_eq!(
            mode(&cutout, Some(&binding(Some(reference)))),
            (AlphaMode::Mask { cutoff: reference }, vec![])
        );
        for (compared, policy) in [
            (
                primase_core::shader::AlphaComparison::Dithered { scale_word: 21 },
                DITHERED_ALPHA_CUT,
            ),
            (
                primase_core::shader::AlphaComparison::Transformed,
                TRANSFORMED_ALPHA_CUT,
            ),
        ] {
            let mut qualified = binding(Some(reference));
            qualified.stages[0].constants[0].role = ConstantRole::AlphaReference {
                value: reference,
                compared,
            };
            assert_eq!(
                mode(&cutout, Some(&qualified)),
                (AlphaMode::Mask { cutoff: reference }, vec![policy.to_owned()])
            );
        }
        for r in [0., -5. / 255., -100.] {
            assert_eq!(
                mode(&cutout, Some(&binding(Some(r)))),
                (AlphaMode::Opaque, vec![]),
                "{r}"
            );
        }
        assert_eq!(
            mode(&cutout, Some(&binding(None))),
            (AlphaMode::Opaque, vec![UNTESTED_ALPHA_REFERENCE.to_owned()])
        );
        // the program compares a value it never writes (no reference stated), and the lowering colours the surface with
        // the slot it read as the tested one: the material's reference cuts it, counted
        assert_eq!(
            mode_read(&cutout, Some(&binding(None)), true),
            (
                AlphaMode::Mask { cutoff: reference },
                vec![COVERAGE_ALPHA_CUT.to_owned()]
            )
        );
        // a program whose discard compares a value it writes nowhere: the binding says so (`Unwritten`), and the cut is the
        // program's own expression solved by the lowering (`Solved`), uncounted, `None` where it discards nothing; where
        // the lowering did not solve it nothing is cut and the count says so
        let unwritten = |m: &Material, b: Option<&ShaderBinding>, read: AlphaTestRead| {
            let mut log = primase_core::pass::PassLog::default();
            let mode = alpha_mode_read(m, b, read, &mut log);
            (mode, log.counts.keys().cloned().collect::<Vec<_>>())
        };
        let mut unwritten_binding = binding(Some(reference));
        unwritten_binding.stages[0].constants[0].role = ConstantRole::AlphaReference {
            value: reference,
            compared: primase_core::shader::AlphaComparison::Unwritten,
        };
        let solved = |cut, dithered| AlphaTestRead::Solved { cut, dithered };
        // scale 0: the dither term vanished and the cut is exact, uncounted; a nonzero scale (0.5) counts DITHERED
        assert_eq!(
            unwritten(&cutout, Some(&unwritten_binding), solved(Some(0.25), false)),
            (AlphaMode::Mask { cutoff: 0.25 }, vec![])
        );
        assert_eq!(
            unwritten(&cutout, Some(&unwritten_binding), solved(Some(0.25), true)),
            (
                AlphaMode::Mask { cutoff: 0.25 },
                vec![DITHERED_ALPHA_CUT.to_owned()]
            )
        );
        assert_eq!(
            unwritten(&cutout, Some(&unwritten_binding), solved(None, false)),
            (AlphaMode::Opaque, vec![])
        );
        for read in [AlphaTestRead::Unread, AlphaTestRead::Coverage] {
            assert_eq!(
                unwritten(&cutout, Some(&unwritten_binding), read),
                (AlphaMode::Opaque, vec![UNSOLVED_ALPHA_TEST.to_owned()]),
                "{read:?}"
            );
        }
        // a solved test is the cut with no binding too
        assert_eq!(
            unwritten(&cutout, None, solved(Some(0.25), false)),
            (AlphaMode::Mask { cutoff: 0.25 }, vec![])
        );
        // the engine's two words of the alpha constant: the material's over 255, -100 and 0 where it states none
        assert_eq!(engine_alpha_reference(&cutout), [reference, 0.]);
        assert_eq!(
            engine_alpha_reference(&material("cutout", None, 0.5)),
            [-100., 0.]
        );
        assert_eq!(
            engine_alpha_reference(&material("cutout", Some(0), 0.5)),
            [-100., 0.]
        );
        // a reference the binding states, or none the material states, is decided as before
        assert_eq!(
            mode_read(&cutout, Some(&binding(Some(reference))), true),
            (AlphaMode::Mask { cutoff: reference }, vec![])
        );
        assert_eq!(
            mode_read(&material("cutout", Some(0), 0.5), Some(&binding(None)), true),
            (AlphaMode::Opaque, vec![])
        );
        let plain = material("opaque", Some(0), 0.5);
        assert_eq!(mode(&plain, Some(&binding(None))), (AlphaMode::Opaque, vec![]));
        assert_eq!(
            mode(&cutout, None),
            (
                AlphaMode::Mask { cutoff: reference },
                vec![UNBOUND_ALPHA_CUT.to_owned()]
            )
        );
        assert_eq!(mode(&plain, None), (AlphaMode::Opaque, vec![]));
        assert_eq!(
            mode(
                &material("alpha", Some(128), 0.5),
                Some(&binding(Some(reference)))
            )
            .0,
            AlphaMode::Blend
        );
        assert_eq!([alpha_reference(0), alpha_reference(-5)], [None, None]);
    }
}
