//! The detail-blend equation read from Sekiro's own programs ([`primase_fromsoft::detail_derive`]): the switch 0, 13, 6
//! and 1 programs (one, two and three details; switch 1's two weighed by the base normal's z) prove with a material's
//! words bound as the bake binds them, and the programs of another form are refused naming what differs. Needs the Sekiro shader corpus (`oracles.corpus/sekiro/shaders`);
//! skipped without it. `derive-fromsoft-detail` on the Wolf's outfits read from the install needs the Sekiro install and
//! Oodle; skipped without them.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::detail::{GlossScale, ReflectanceTerm, WeightStep};
use primase_fromsoft::detail_derive::{BOUND_UNITS, DetailSlots, derive};
use primase_fromsoft::detail_probe::ProgramProbe;
use primase_fromsoft::materials::LayeredMaterial;
use primase_fromsoft::mtd::{MaterialDefinition, Parameter, Value};

type TestResult = Result<(), Box<dyn std::error::Error>>;
mod common;

/// The program `name` (`.ppo`) anywhere under the corpus `root`.
fn find(root: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name)? {
                return Ok(Some(found));
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The Sekiro program `name`, or `None` without the corpus; an error where the corpus lacks it.
fn program(name: &str) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::oracle(
        primase_testdata::OracleSet::Corpus,
        primase_testdata::Game::Sekiro,
        "shaders",
    ) else {
        return Ok(None);
    };
    let path = find(&root, name)?.ok_or_else(|| format!("{name} is not in the shader corpus"))?;
    Ok(Some(std::fs::read(path)?))
}

const FIELD: &str = "Character_AMSN__DetailBlend__snp_0_";

/// A DetailBlend material stating every field the switch 0, 13, 6 and 1 equations read, its second detail's colour as
/// `P_LG_M_9000_bottoms1.mtd` states it (1 at an intensity of 1.2), its first's at an intensity of 3 and its third's at
/// 2 (switch 1's first detail's colour is `color_1`, at 2.5).
fn material() -> LayeredMaterial {
    let floats = |v: &[f32]| Value::Float(v.to_vec());
    let parameters = [
        (format!("{FIELD}color_0"), floats(&[0.8, 0.7, 0.6])),
        (format!("{FIELD}color_0-Intensity"), floats(&[3.])),
        (format!("{FIELD}color_5"), floats(&[1., 1., 1.])),
        (format!("{FIELD}color_5-Intensity"), floats(&[1.2])),
        (format!("{FIELD}color_2"), floats(&[0.4, 0.5, 0.6])),
        (format!("{FIELD}color_2-Intensity"), floats(&[2.])),
        (format!("{FIELD}float_0"), floats(&[0.25])),
        (format!("{FIELD}float_15"), floats(&[-0.1])),
        (format!("{FIELD}float_13"), floats(&[0.125])),
        (format!("{FIELD}float_2"), floats(&[0.5])),
        (format!("{FIELD}float_16"), floats(&[0.75])),
        (format!("{FIELD}float_5"), floats(&[0.375])),
        (format!("{FIELD}float_10"), floats(&[0.6])),
        (format!("{FIELD}color_1"), floats(&[0.3, 0.4, 0.5])),
        (format!("{FIELD}color_1-Intensity"), floats(&[2.5])),
        (format!("{FIELD}float_1"), floats(&[0.2])),
        (format!("{FIELD}float_8"), floats(&[0.45])),
        ("g_DiffuseMapColor".to_owned(), floats(&[0.9, 0.85, 0.8])),
    ];
    LayeredMaterial {
        mtd: "detail_derive_test.mtd".into(),
        definition: MaterialDefinition {
            shader: r"N:\NTC\data\Material\sat\OutputData\SPX\Character_AMSN_[DetailBlend].spx".into(),
            description: String::new(),
            parameters: parameters
                .into_iter()
                .map(|(name, value)| Parameter { name, value })
                .collect(),
            textures: Vec::new(),
            block_lengths: Vec::new(),
        },
        bindings: Default::default(),
    }
}

/// Switches 0, 13, 6 and 1, one, two and three details (plain and with the decal textures of `_Paint`): the textures'
/// parts read from the program, every constant solved and proven (on the random sets and where the weights mix), each
/// the word it reads as the binding writes it, the reflectance term reading the base mask the first detail's (switch 1's
/// second's), each gloss term scaled by the base normal's z (switch 1's first by the program's 0.5), and no alpha (the
/// programs' alpha lane has no role). The second detail's tint of 13 and 6 is the gain (4.55, measured) times the colour
/// times its intensity, so the gain is measured on top of the intensity, never folded with it.
#[test]
fn one_two_and_three_details_prove_with_the_materials_words() -> TestResult {
    let detail = |colour, normal| DetailSlots { colour, normal };
    // the one-detail program's base and detail colour enter only as a product; at the same tiling (the material states
    // none) the detail is the one it samples first
    let cases = [
        ("0", vec![detail(0, 11)], 0),
        ("13", vec![detail(0, 11), detail(3, 5)], 0),
        ("6", vec![detail(0, 11), detail(3, 5), detail(9, 13)], 0),
        ("1", vec![detail(7, 10), detail(0, 11)], 1),
    ];
    for (switch, details, masked) in cases {
        for variant in ["", "_Paint"] {
            let name = format!("Character_AMSN__DetailBlend__{switch}{variant}_Gbuf.ppo");
            let Some(bytes) = program(&name)? else {
                return Ok(());
            };
            let mut probe = ProgramProbe::with_material(&bytes, &material())?;
            let d = derive(&mut probe, 7).map_err(|e| format!("{name}: {e}"))?;
            assert_eq!(
                (
                    d.equation.roles.base_colour,
                    d.equation.roles.base_normal,
                    d.equation.roles.base_mask
                ),
                (1, 2, 8),
                "{name}"
            );
            assert_eq!(d.equation.roles.details, details, "{name}");
            assert!(
                d.equation.proof.largest.values().all(|&u| u <= BOUND_UNITS),
                "{name}: {:?}",
                d.equation.proof.largest
            );
            match &d.equation.proof.transition {
                None => assert_eq!(details.len(), 1, "{name}: no sets where the weights mix"),
                Some((sets, largest)) => {
                    assert!(
                        *sets > 0 && largest.values().all(|&u| u <= BOUND_UNITS),
                        "{name}: {largest:?}"
                    )
                }
            }
            for (j, t) in d.equation.constants.details.iter().enumerate() {
                assert_eq!(
                    matches!(t.reflectance, ReflectanceTerm::MaskPlusBias(_)),
                    j == masked,
                    "{name}: detail {j}'s reflectance term {:?}",
                    t.reflectance
                );
                let scale = if switch == "1" && j == 0 {
                    GlossScale::Constant(0.5)
                } else {
                    GlossScale::BaseNormalZ
                };
                assert_eq!(t.gloss_scale, scale, "{name}: detail {j}'s gloss scale");
            }
            if switch == "1" {
                // min((22 N.z)^260, 1)
                let lerp = d.equation.detail_lerp(|_| {
                    primase_core::pbr::TextureRef::new(primase_core::World::default().spawn())
                });
                assert_eq!(
                    lerp.weights[0].steps,
                    [WeightStep::Power(260), WeightStep::Min(1.)],
                    "{name}"
                );
            }
            assert_eq!(
                d.equation.constants.alpha, None,
                "{name}: the alpha lane has no role"
            );
            let disagreements: Vec<_> = d.disagreements().collect();
            assert!(disagreements.is_empty(), "{name}: {disagreements:?}");
            // every constant but the dielectric reflectance reads one word; that is the program's own
            for f in &d.fields {
                match f.constant.as_str() {
                    "dielectric_f0" => assert!(f.fields.is_empty() && f.solved == 0.03, "{name}: {f:?}"),
                    "gloss_scale[0]" => assert!(f.fields.is_empty() && f.solved == 0.5, "{name}: {f:?}"),
                    _ => assert!(f.fields.len() == 1 && f.agrees() == Some(true), "{name}: {f:?}"),
                }
            }
            let fold = d.fold.ok_or("the fold was not measured")?;
            assert!(
                (fold.gain - 4.55).abs() <= 4. * f32::EPSILON * 4.55,
                "{name}: gain {}",
                fold.gain
            );
            assert!(fold.largest <= BOUND_UNITS, "{name}: fold {}", fold.largest);
            if details.len() >= 2 && switch != "1" {
                // the binding writes the colour times its intensity, and the gain is measured on top of it
                let field = probe
                    .material
                    .iter()
                    .find(|m| m.name == format!("{FIELD}color_5"))
                    .ok_or("no color_5 field")?
                    .clone();
                let words = probe
                    .words(field.buffer, field.word, 3)
                    .ok_or("color_5 past its buffer")?;
                assert_eq!(words, vec![1.2f32.to_bits(); 3], "{name}: color_5 as bound");
                let tint1 = d
                    .fields
                    .iter()
                    .find(|f| f.constant == "tint[1][0]")
                    .ok_or("no tint[1][0]")?;
                assert_eq!(tint1.fields, vec![(format!("{FIELD}color_5"), 0)], "{name}");
                assert!(
                    (d.equation.constants.details[1].tint[0] - 4.55 * 1.2).abs() <= 8. * f32::EPSILON * 5.46,
                    "{name}: tint {:?}",
                    d.equation.constants.details[1].tint
                );
            }
        }
    }
    Ok(())
}

/// The programs of another form are refused, each naming its difference from the equation.
#[test]
fn other_forms_are_refused_naming_the_difference() -> TestResult {
    for (name, reason) in [
        (
            "Character_AMSN__DetailBlend__14_Gbuf.ppo",
            "the base colour multiplies one detail only",
        ),
        (
            "Character_AMSN__DetailBlend__4_Gbuf.ppo",
            "the base colour multiplies one detail only",
        ),
        (
            "Character_AMSN__DetailBlend__2_Gbuf.ppo",
            "the base colour multiplies one detail only",
        ),
        (
            "Character_AMSN__DetailBlend__3_Gbuf.ppo",
            "the reflectance term of detail 2 reads the base mask",
        ),
        (
            "Character_AMSN__DetailBlend__13_PhantomLite_Gbuf.ppo",
            "(FC_PhantomDiffColor)",
        ),
        (
            "Character_AMSN__DetailBlend__0_PhantomLite_Gbuf.ppo",
            "(FC_PhantomDiffColor)",
        ),
    ] {
        let Some(bytes) = program(name)? else {
            return Ok(());
        };
        let mut probe = ProgramProbe::with_material(&bytes, &material())?;
        match derive(&mut probe, 7) {
            Ok(d) => panic!("{name} derived: {:?}", d.equation.roles),
            Err(e) => assert!(e.contains(reason), "{name}: {e}"),
        }
    }
    Ok(())
}

/// A world holding `name`'s program as `resolve-fromsoft-shaders` leaves it for one material: the material (its
/// shader Sekiro's `Character_AMSN_[DetailBlend]`), the program, and the binding with a texture entity in every slot
/// (the probe's own resolution, each slot given a texture, its sampler stating no addressing), and a triangle drawing
/// the material at coordinates `uv`. The material's entity, or `None` without the corpus.
fn bound_world(
    world: &mut primase_core::World,
    name: &str,
    uv: [[f32; 2]; 3],
) -> Result<Option<primase_core::Entity>, Box<dyn std::error::Error>> {
    use primase_core::shader::{SamplerState, Shader, ShaderBinding, ShaderFormat, TextureSource};
    use primase_fromsoft::materials::{Definition, Material};
    let Some(bytes) = program(name)? else {
        return Ok(None);
    };
    let layered = material();
    let probe = ProgramProbe::with_material(&bytes, &layered)?;
    let shader = world.spawn();
    world.insert(
        shader,
        Shader {
            format: ShaderFormat::Dxbc,
            bytecode: bytes.clone(),
            interface: probe.interface().clone(),
        },
    );
    let mut stage = probe.binding.clone();
    stage.shader = shader;
    for t in &mut stage.textures {
        let texture = world.spawn();
        world.insert(
            texture,
            primase_core::components::TextureFile {
                path: format!("t{}.png", t.resource.0).into(),
            },
        );
        t.source = TextureSource::Texture {
            texture,
            sampler: SamplerState::default(),
        };
    }
    let e = world.spawn();
    world.insert(
        e,
        ShaderBinding {
            stages: vec![stage],
            state: Default::default(),
        },
    );
    let mesh = world.spawn();
    world.insert(
        mesh,
        primase_core::Mesh::new(vec![primase_core::Primitive {
            topology: primase_core::Topology::Triangles,
            streams: [
                (
                    primase_core::Semantic::Position,
                    primase_core::Stream::F32x3(vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]]),
                ),
                (
                    primase_core::Semantic::TexCoord(0),
                    primase_core::Stream::F32x2(uv.to_vec()),
                ),
            ]
            .into(),
            indices: None,
            material: Some(e),
        }]),
    );
    world.insert(
        e,
        Material {
            mtd: layered.mtd.clone(),
            definition: Some(Definition::Mtd(layered.definition.clone())),
            gx: Vec::new(),
            shader: "Character_AMSN_[DetailBlend]".into(),
            class: "opaque".into(),
            skip: None,
            cutoff: 0.5,
            bindings: layered.bindings.clone(),
            layered: Some(layered),
        },
    );
    Ok(Some(e))
}

/// A triangle's coordinates within 0 to 1.
const UNIT_TRIANGLE: [[f32; 2]; 3] = [[0., 0.], [1., 0.], [0., 1.]];

/// The slots the pass counted as repeating where the source states no addressing, by count.
fn wraps(log: &primase_core::pass::PassLog) -> Vec<(String, u64)> {
    log.approximations
        .iter()
        .filter(|(k, _)| k.starts_with("wrap not stated"))
        .map(|(k, n)| (k.clone(), *n))
        .collect()
}

/// `derive-fromsoft-detail` gives a detail-blend material whose program proves its `pbr.detail`: the equation the
/// derivation reads, each slot the binding's texture at the program's tiling on the one coordinate set, the facing among
/// what it holds constant; one whose program does not prove gets none, counted by its reason.
#[test]
fn the_pass_gives_a_proven_detail_material_and_counts_the_rest() -> TestResult {
    use primase_core::detail::{DetailMix, DetailedMaterial, Frozen};
    use primase_core::pass::Pass;
    use primase_fromsoft::sekiro::passes::DeriveFromSoftDetail;
    use primase_fromsoft::sekiro::passes::detail::UNSTATED_WRAP_UNIT;
    for (switch, details) in [("13", 2), ("1", 2), ("6", 3)] {
        let name = format!("Character_AMSN__DetailBlend__{switch}_Gbuf.ppo");
        let mut world = primase_core::World::default();
        let Some(e) = bound_world(&mut world, &name, UNIT_TRIANGLE)? else {
            return Ok(());
        };
        let mut log = primase_core::pass::PassLog::default();
        DeriveFromSoftDetail.run(&mut world, &mut log)?;
        let detail = world
            .get::<DetailedMaterial>(e)
            .ok_or_else(|| format!("{name}: no pbr.detail ({:?})", log.counts))?;
        let DetailMix::DetailLerp(lerp) = &detail.mix;
        assert_eq!(lerp.details.len(), details, "{name}");
        assert_eq!(lerp.alpha, None, "{name}");
        for t in lerp.textures() {
            assert_eq!(t.texcoord, 0, "{name}");
            assert!(t.transform.is_some(), "{name}: the tiling");
            assert!(world.is_alive(t.texture), "{name}");
        }
        assert!(
            detail.frozen.contains(&Frozen::FrontFacing),
            "{name}: {:?}",
            detail.frozen
        );
        // the equation is the derivation's, on the material's words
        let mut probe = ProgramProbe::with_material(&program(&name)?.unwrap(), &material())?;
        let seed = primase_fromsoft::sekiro::passes::detail::proof_seed(&material().mtd);
        let equation = primase_fromsoft::detail_derive::derive_equation(&mut probe, seed)?;
        assert_eq!(lerp.weights.len(), equation.roles.details.len() - 1, "{name}");
        assert_eq!(
            lerp.details.iter().map(|d| d.tint).collect::<Vec<_>>(),
            equation
                .constants
                .details
                .iter()
                .map(|d| d.tint)
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            log.counts.get(
                "detail-blend materials given pbr.detail (their program's equation proven on their own words)"
            ),
            Some(&1),
            "{name}"
        );
        // no sampler states addressing and the material no tiling: every slot (the base's three, each detail's two)
        // samples the triangle's coordinates as they are, within 0 to 1
        assert_eq!(
            wraps(&log),
            [(UNSTATED_WRAP_UNIT.to_owned(), 3 + 2 * details as u64)],
            "{name}"
        );
    }
    // the triangle reaching u = 2: one coordinate of three past 1, in every slot
    let name = "Character_AMSN__DetailBlend__13_Gbuf.ppo";
    let mut world = primase_core::World::default();
    if bound_world(&mut world, name, [[0., 0.], [2., 0.], [0., 1.]])?.is_some() {
        let mut log = primase_core::pass::PassLog::default();
        DeriveFromSoftDetail.run(&mut world, &mut log)?;
        assert_eq!(
            wraps(&log),
            [(
                "wrap not stated; Clamp would hold the texture constant beyond 0 to 1 over 34% of the coordinates"
                    .to_owned(),
                7
            )],
            "{name}"
        );
    }
    // the triangle's coordinates multiplied by a tiling of 2 the geometry pass applied: each slot's transform halves,
    // and the coordinates it maps stay within 0 to 1
    let mut unit = primase_core::World::default();
    let mut tiled = primase_core::World::default();
    if let (Some(a), Some(b)) = (
        bound_world(&mut unit, name, UNIT_TRIANGLE)?,
        bound_world(&mut tiled, name, [[0., 0.], [2., 0.], [0., 2.]])?,
    ) {
        tiled.insert(b, primase_fromsoft::sekiro::passes::UvTiling([2.; 2]));
        let mut log = primase_core::pass::PassLog::default();
        DeriveFromSoftDetail.run(&mut unit, &mut primase_core::pass::PassLog::default())?;
        DeriveFromSoftDetail.run(&mut tiled, &mut log)?;
        assert_eq!(wraps(&log), [(UNSTATED_WRAP_UNIT.to_owned(), 7)], "{name} tiled");
        let textures = |w: &primase_core::World, e| {
            w.get::<DetailedMaterial>(e).map(|d| {
                let DetailMix::DetailLerp(lerp) = &d.mix;
                lerp.textures()
                    .into_iter()
                    .map(|t| t.transform)
                    .collect::<Vec<_>>()
            })
        };
        let (Some(at_unit), Some(at_tiled)) = (textures(&unit, a), textures(&tiled, b)) else {
            return Err(format!("{name}: no pbr.detail").into());
        };
        let halved: Vec<_> = at_unit
            .into_iter()
            .map(|t| {
                t.map(|t| primase_core::pbr::UvTransform {
                    scale: t.scale.map(|v| v / 2.),
                    ..t
                })
            })
            .collect();
        assert_eq!(at_tiled, halved, "{name} tiled");
    }
    let name = "Character_AMSN__DetailBlend__14_Gbuf.ppo";
    let mut world = primase_core::World::default();
    let Some(e) = bound_world(&mut world, name, UNIT_TRIANGLE)? else {
        return Ok(());
    };
    let mut log = primase_core::pass::PassLog::default();
    DeriveFromSoftDetail.run(&mut world, &mut log)?;
    assert!(world.get::<primase_core::detail::DetailedMaterial>(e).is_none());
    assert!(
        log.counts.keys().any(|k| k.starts_with(
            "detail-blend materials given no pbr.detail: no colour texture moves the colour at both ends"
        )),
        "{:?}",
        log.counts
    );
    Ok(())
}

/// The Wolf's default outfit (`CharaInitParam` 9000's parts) and the 'Another's Memory: Ashina' costume
/// (`EquipParamProtector` 400000 to 403000's).
const OUTFITS: [(&str, &[&str]); 2] = [
    (
        "Wolf (default outfit)",
        &["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"],
    ),
    (
        "Another's Memory: Ashina",
        &[
            "fc_m_0220_l",
            "hd_m_9120_l",
            "bd_m_9120_l",
            "am_m_9120_l",
            "lg_m_9120_l",
        ],
    ),
];

/// What `derive-fromsoft-detail` did to one outfit.
#[derive(Debug, PartialEq, Default)]
struct DetailCensus {
    /// The detail-blend materials.
    candidates: usize,
    /// Those given a `pbr.detail`, by MTD.
    given: Vec<String>,
    /// Those given none, by reason.
    refused: BTreeMap<String, u64>,
    /// Slots counted as repeating where the source states no addressing, by reason.
    wraps: BTreeMap<String, u64>,
}

/// The reason a slot the equation reads has no texture: the binding reads a constant, the preview colour of a
/// missing `binding` map.
fn unbound(slot: u32, role: &str, binding: &str) -> String {
    format!(
        "slot t{slot}, the {role} the equation reads, has no texture: the binding reads a constant (preview default \
         for missing {binding} binding)"
    )
}

/// The wrap counts of `n` slots at each share past 0 to 1 (`percent`), and `within` slots inside it.
fn wrap_counts(within: u64, beyond: &[(u32, u64)]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    if within > 0 {
        out.insert(
            primase_fromsoft::sekiro::passes::detail::UNSTATED_WRAP_UNIT.to_owned(),
            within,
        );
    }
    for (percent, n) in beyond {
        out.insert(
            format!(
                "wrap not stated; Clamp would hold the texture constant beyond 0 to 1 over {percent}% of the coordinates"
            ),
            *n,
        );
    }
    out
}

/// The outfits read from the install and lowered as the lab lowers a character up to its shader binding
/// (`lower-fromsoft-geometry` keeping the authored coordinates, `resolve-fromsoft-shaders`), then
/// `derive-fromsoft-detail`: which detail-blend materials get a `pbr.detail`, the rest by reason, and the slots whose
/// addressing the source does not state, by whether their coordinates stay within 0 to 1.
#[test]
fn the_pass_on_the_wolfs_outfits() -> TestResult {
    use primase_core::detail::DetailedMaterial;
    use primase_fromsoft::materials::Material;
    let (Some(root), Some(dll)) = (
        primase_testdata::install(primase_testdata::Game::Sekiro),
        primase_testdata::oodle(primase_testdata::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = primase_fromsoft::dcx::Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut census = BTreeMap::new();
    for (outfit, parts) in OUTFITS {
        let mut world = primase_core::World::default();
        primase_fromsoft::sekiro::read_parts(&root, parts, &decoder, &mut world)?;
        let report = common::run(
            &mut world,
            "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}, \
             resolve-fromsoft-shaders{unbound-textures=preview}, derive-fromsoft-detail",
        )?;
        let log = &report.steps.last().ok_or("no steps")?.log;
        let mut c = DetailCensus::default();
        for (e, m) in world.iter::<Material>() {
            if m.shader == "Character_AMSN_[DetailBlend]" {
                c.candidates += 1;
                if world.get::<DetailedMaterial>(e).is_some() {
                    c.given.push(m.mtd.clone());
                }
            }
        }
        c.given.sort();
        let given =
            "detail-blend materials given pbr.detail (their program's equation proven on their own words)";
        assert_eq!(
            log.counts.get(given).copied().unwrap_or(0),
            c.given.len() as u64,
            "{outfit}"
        );
        let refused = "detail-blend materials given no pbr.detail: ";
        c.refused = log
            .counts
            .iter()
            .filter_map(|(k, n)| Some((k.strip_prefix(refused)?.to_owned(), *n)))
            .collect();
        c.wraps = log
            .approximations
            .iter()
            .filter(|(k, _)| k.starts_with("wrap not stated"))
            .map(|(k, n)| (k.clone(), *n))
            .collect();
        println!("{outfit}: {c:#?}");
        census.insert(outfit, c);
    }
    // the counts of the full-install run on adbd9051 (the Wolf's default outfit; the Ashina costume), which the pass
    // then counted by the reasons it has now; since the dithered alpha test is recognized and frozen off (0f0c5798),
    // the materials a program's discard stopped at (3 of the default outfit, 1 of the costume: the dithered fade) are refused by the next
    // reason each meets, an unbound AlbedoMap or MetallicMap, and their slots' wraps are counted (with that commit
    // reverted the discard counts are 3 and 1 again)
    let expected = BTreeMap::from([
        (
            OUTFITS[0].0,
            (
                9,
                vec!["P_AM_M_9000_artificialarm", "P_LG_M_9000_bottoms1"],
                BTreeMap::from([
                    (unbound(0, "detail 0 colour", "AlbedoMap"), 3),
                    (unbound(8, "base mask", "MetallicMap"), 4),
                ]),
                wrap_counts(7, &[(11, 7), (23, 7), (51, 3), (98, 1), (99, 1), (100, 6)]),
            ),
        ),
        (
            OUTFITS[1].0,
            (
                8,
                vec!["P_AM_M_9120_artificialarm", "P_BD_M_9120_haori"],
                BTreeMap::from([
                    (unbound(0, "detail 0 colour", "AlbedoMap"), 5),
                    (unbound(8, "base mask", "MetallicMap"), 1),
                ]),
                wrap_counts(0, &[(11, 9), (27, 7), (70, 3), (100, 1)]),
            ),
        ),
    ]);
    for (outfit, c) in &census {
        let (candidates, given, refused, wraps) = &expected[outfit];
        assert_eq!(c.candidates, *candidates, "{outfit}");
        assert_eq!(
            c.given
                .iter()
                .map(|m| m.trim_end_matches(".mtd"))
                .collect::<Vec<_>>(),
            *given,
            "{outfit}"
        );
        assert_eq!(&c.wraps, wraps, "{outfit}");
        // every material not given one is counted once, by a reason that is complete text: no Debug of a type, no
        // value of the material
        assert_eq!(&c.refused, refused, "{outfit}");
        assert_eq!(
            c.refused.values().sum::<u64>(),
            (c.candidates - c.given.len()) as u64,
            "{outfit}: {:?}",
            c.refused
        );
        assert!(
            c.refused
                .keys()
                .all(|k| !k.contains('{') && !k.contains('}') && !k.trim().is_empty()),
            "{outfit}: {:?}",
            c.refused
        );
    }
    Ok(())
}
