//! The detail-blend equation read from Sekiro's own programs ([`primase_fromsoft::detail_derive`]): the switch 0, 13, 6
//! and 1 programs (one, two and three details; switch 1's two weighed by the base normal's z) prove with a material's
//! words bound as the bake binds them, and the programs of another form are refused naming what differs. Needs the Sekiro shader corpus (`oracles.corpus/sekiro/shaders`);
//! skipped without it.
use std::path::{Path, PathBuf};

use primase_core::detail::{GlossScale, ReflectanceTerm, WeightStep};
use primase_fromsoft::detail_derive::{BOUND_UNITS, DetailSlots, derive};
use primase_fromsoft::detail_probe::ProgramProbe;
use primase_fromsoft::materials::LayeredMaterial;
use primase_fromsoft::mtd::{MaterialDefinition, Parameter, Value};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
                (d.roles.base_colour, d.roles.base_normal, d.roles.base_mask),
                (1, 2, 8),
                "{name}"
            );
            assert_eq!(d.roles.details, details, "{name}");
            assert!(
                d.proof.largest.values().all(|&u| u <= BOUND_UNITS),
                "{name}: {:?}",
                d.proof.largest
            );
            match &d.proof.transition {
                None => assert_eq!(details.len(), 1, "{name}: no sets where the weights mix"),
                Some((sets, largest)) => {
                    assert!(
                        *sets > 0 && largest.values().all(|&u| u <= BOUND_UNITS),
                        "{name}: {largest:?}"
                    )
                }
            }
            for (j, t) in d.constants.details.iter().enumerate() {
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
                let lerp = d.detail_lerp(|_| {
                    primase_core::pbr::TextureRef::new(primase_core::World::default().spawn())
                });
                assert_eq!(
                    lerp.weights[0].steps,
                    [WeightStep::Power(260), WeightStep::Min(1.)],
                    "{name}"
                );
            }
            assert_eq!(d.constants.alpha, None, "{name}: the alpha lane has no role");
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
                    (d.constants.details[1].tint[0] - 4.55 * 1.2).abs() <= 8. * f32::EPSILON * 5.46,
                    "{name}: tint {:?}",
                    d.constants.details[1].tint
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
            Ok(d) => panic!("{name} derived: {:?}", d.roles),
            Err(e) => assert!(e.contains(reason), "{name}: {e}"),
        }
    }
    Ok(())
}
