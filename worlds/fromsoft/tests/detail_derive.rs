//! The detail-blend equation read from Sekiro's own programs ([`primase_fromsoft::detail_derive`]): the switch-13
//! programs prove with a material's words bound as the bake binds them, and the programs of another form are refused
//! naming what differs. Needs the Sekiro shader corpus (`oracles.corpus/sekiro/shaders`); skipped without it.
use std::path::{Path, PathBuf};

use primase_fromsoft::detail_derive::{BOUND_UNITS, derive};
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

/// A DetailBlend material stating every field the switch-13 equation reads, its second detail's colour as
/// `P_LG_M_9000_bottoms1.mtd` states it (1 at an intensity of 1.2) and its first's at an intensity of 3.
fn material() -> LayeredMaterial {
    let floats = |v: &[f32]| Value::Float(v.to_vec());
    let parameters = [
        (format!("{FIELD}color_0"), floats(&[0.8, 0.7, 0.6])),
        (format!("{FIELD}color_0-Intensity"), floats(&[3.])),
        (format!("{FIELD}color_5"), floats(&[1., 1., 1.])),
        (format!("{FIELD}color_5-Intensity"), floats(&[1.2])),
        (format!("{FIELD}float_0"), floats(&[0.25])),
        (format!("{FIELD}float_15"), floats(&[-0.1])),
        (format!("{FIELD}float_2"), floats(&[0.5])),
        (format!("{FIELD}float_16"), floats(&[0.75])),
        (format!("{FIELD}float_10"), floats(&[0.6])),
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

/// Switch 13 (plain and with the decal textures of `_Paint`): the textures' parts read from the program, every constant
/// solved and proven, each the word it reads as the binding writes it. The second tint is the gain (4.55, measured) times
/// the colour times its intensity, so the gain is measured on top of the intensity, never folded with it.
#[test]
fn switch_13_proves_with_the_materials_words() -> TestResult {
    for name in [
        "Character_AMSN__DetailBlend__13_Gbuf.ppo",
        "Character_AMSN__DetailBlend__13_Paint_Gbuf.ppo",
    ] {
        let Some(bytes) = program(name)? else {
            return Ok(());
        };
        let mut probe = ProgramProbe::with_material(&bytes, &material())?;
        // the binding writes the colour times its intensity
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
        let d = derive(&mut probe, 7).map_err(|e| format!("{name}: {e}"))?;
        assert_eq!(
            (d.roles.base_colour, d.roles.base_normal, d.roles.base_mask),
            (1, 2, 8),
            "{name}"
        );
        assert_eq!(
            (d.roles.detail_colour, d.roles.detail_normal),
            ([0, 3], [11, 5]),
            "{name}"
        );
        assert!(
            d.proof.largest.values().all(|&u| u <= BOUND_UNITS),
            "{name}: {:?}",
            d.proof.largest
        );
        let disagreements: Vec<_> = d.disagreements().collect();
        assert!(disagreements.is_empty(), "{name}: {disagreements:?}");
        // every constant but the dielectric reflectance reads one word; that is the program's own
        for f in &d.fields {
            match f.constant.as_str() {
                "dielectric_f0" => assert!(f.fields.is_empty() && f.solved == 0.03, "{name}: {f:?}"),
                _ => assert!(f.fields.len() == 1 && f.agrees() == Some(true), "{name}: {f:?}"),
            }
        }
        let tint1 = d
            .fields
            .iter()
            .find(|f| f.constant == "tint[1][0]")
            .ok_or("no tint[1][0]")?;
        assert_eq!(tint1.fields, vec![(format!("{FIELD}color_5"), 0)], "{name}");
        let fold = d.fold.ok_or("the fold was not measured")?;
        assert!(
            (fold.gain - 4.55).abs() <= 4. * f32::EPSILON * 4.55,
            "{name}: gain {}",
            fold.gain
        );
        assert!(fold.largest <= BOUND_UNITS, "{name}: fold {}", fold.largest);
        assert!(
            (d.constants.tint[1][0] - 4.55 * 1.2).abs() <= 8. * f32::EPSILON * 5.46,
            "{name}: tint {:?}",
            d.constants.tint
        );
    }
    Ok(())
}

/// The programs of another form are refused, each naming its difference from the switch-13 equation.
#[test]
fn other_forms_are_refused_naming_the_difference() -> TestResult {
    for (name, reason) in [
        (
            "Character_AMSN__DetailBlend__14_Gbuf.ppo",
            "the base colour multiplies one detail only",
        ),
        (
            "Character_AMSN__DetailBlend__1_Gbuf.ppo",
            "the mask is not the base normal's (1 - N.a)^8",
        ),
        (
            "Character_AMSN__DetailBlend__2_Gbuf.ppo",
            "the mask is not the base normal's (1 - N.a)^8",
        ),
        (
            "Character_AMSN__DetailBlend__13_PhantomLite_Gbuf.ppo",
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
