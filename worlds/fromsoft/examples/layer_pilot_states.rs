//! The layer-bake pilot's inputs for another executor: the pipeline states ([`primase_fromsoft::layer_bake::PipelineState`])
//! of a few layered materials of an area, each bake exactly as the area's bake runs it, written for a rasterizer to run
//! the same programs on (the private WARP host's `quad` mode; `tests/layer_pilot.rs` compares).
//!
//! The area is read and lowered as the conversion does up to the shader binding (`resolve-fromsoft-shaders`); its bake
//! groups ([`primase_fromsoft::sekiro::passes::layers::AreaBake`]) are taken in their order, and the first group of
//! each kind is kept until `--cases` are: the shader family (`M[Multiple]`, `M[MultipleGround]`,
//! `M[Multiple]_Emissive`), whether the material has a vertex program, and whether its program tests alpha. Each case
//! is written at each `--caps` size cap and in three layer-weight variants: 0 the defaults, 1 mixed weights and a tint,
//! 2 every weight one and detail zero.
//!
//! `cargo run --release -p primase-fromsoft --example layer_pilot_states -- <Sekiro install> <oo2core_6_win64.dll>
//! <out> [--area m11_01] [--region west,south,east,north] [--cases 5] [--caps 32,128]`. It writes `<out>/cases.json`
//! and, per case, cap and variant, `<out>/<case>-<cap>-<variant>/` holding the state's `state.json` and the files it
//! names (its schema: [`primase_fromsoft::layer_bake::SCHEMA`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use primase_compress::oodle::{Oodle, Version};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{LodChoice, SelectLod};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::layer_bake::{ALPHA_TEST_POLICY, Inputs};
use primase_fromsoft::materials::SkipClass;
use primase_fromsoft::rigid::ApplyRigidSkinning;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::passes::layers::AreaBake;
use primase_fromsoft::sekiro::passes::{
    Cards, Degenerates, DropFromSoftFar, LowerFromSoftGeometry, Pose, ResolveFromSoftShaders, Tiling,
    Unbound, Winding,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The layer weights of each variant, as `tests/layer_pilot.rs` names them.
fn variant(n: usize) -> Inputs {
    match n {
        1 => Inputs {
            weights_snow: [0.25, 0.5, 0.75, 0.25],
            tint_color: [0.8, 0.7, 0.6, 1.],
            detail: 1.,
        },
        2 => Inputs {
            weights_snow: [1.; 4],
            tint_color: [1.; 4],
            detail: 0.,
        },
        _ => Inputs::default(),
    }
}

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let (mut area, mut region, mut cases, mut caps) = ("m11_01".to_owned(), None, 5usize, vec![32u32, 128]);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--area" => area = value()?,
            "--region" => {
                let v = value()?
                    .split(',')
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()?;
                let [w, s, e, n] = v[..] else {
                    return Err("--region west,south,east,north".into());
                };
                region = Some([w, s, e, n]);
            }
            "--cases" => cases = value()?.parse()?,
            "--caps" => caps = value()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [install, dll, out] = paths.as_slice() else {
        return Err(
            "usage: layer_pilot_states <Sekiro install> <oo2core_6_win64.dll> <out> [--area m11_01] \
                    [--region west,south,east,north] [--cases 5] [--caps 32,128]"
                .into(),
        );
    };
    let oodle = load_oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let options = sekiro::Options {
        region: region.map(|rect| sekiro::Region { rect, margin: 30.0 }),
        ..Default::default()
    };
    let (mut world, _) = AreaReader::new(install, &area, decoder, options).read()?;
    // lowered as the conversion lowers it, up to the shader binding the bake reads
    let skip = [SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow].into();
    let lower = Pipeline {
        passes: vec![
            Box::new(DropFromSoftFar),
            Box::new(ApplyRigidSkinning),
            Box::new(LowerFromSoftGeometry {
                pose: Pose::Rigid,
                uv_tiling: Tiling::Apply,
                winding: Winding::Normals,
                degenerates: Degenerates::Drop,
                leave_out: skip,
                blocker_cards: Cards::Record,
            }),
            Box::new(SelectLod {
                choice: LodChoice::Finest,
            }),
            Box::new(ResolveFromSoftShaders::new(Unbound::Preview)),
        ],
    };
    PassManager::new(PassRegistry::new()).run(&mut world, &lower, &mut RunReport::default())?;
    let bake = AreaBake::from_world(&world)?;
    std::fs::create_dir_all(out)?;
    let mut kinds = BTreeSet::new();
    let mut listed = Vec::new();
    for (group, g) in bake.groups.iter().enumerate() {
        if listed.len() == cases {
            break;
        }
        let Some(material) = g.material.layered.as_ref() else {
            continue;
        };
        let prepared = bake.prepare(&world, group, 0.0)?;
        let leaf = material
            .definition
            .shader
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        let family = leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_owned();
        let state = prepared.pipeline_state(prepared.bake_size(caps[0])?, g.period, variant(0))?;
        let kind = (
            family.clone(),
            state.vertex.is_some(),
            prepared.policies.contains_key(ALPHA_TEST_POLICY),
        );
        if !kinds.insert(kind.clone()) {
            continue;
        }
        let case = listed.len();
        let mut written = Vec::new();
        for &cap in &caps {
            let size = prepared.bake_size(cap)?;
            for n in 0..3 {
                let state = prepared.pipeline_state(size, g.period, variant(n))?;
                let encoded = state.encode();
                let folder = out.join(format!("{case}-{cap}-{n}"));
                std::fs::create_dir_all(&folder)?;
                std::fs::write(
                    folder.join("state.json"),
                    serde_json::to_vec_pretty(&encoded.description)?,
                )?;
                for (name, bytes) in &encoded.files {
                    std::fs::write(folder.join(name), bytes)?;
                }
                written.push(serde_json::json!({"cap": cap, "variant": n, "size": size, "folder": folder.file_name().map(|f| f.to_string_lossy().into_owned())}));
            }
        }
        println!("case {case}: {} {kind:?}, period {:?}", material.mtd, g.period);
        listed.push(serde_json::json!({
            "case": case,
            "mtd": material.mtd,
            "shader": material.definition.shader,
            "family": kind.0,
            "vertex_program": kind.1,
            "alpha_test": kind.2,
            "period": g.period,
            "period_exact": g.period_exact,
            "policies": prepared.policies,
            "states": written,
        }));
    }
    std::fs::write(out.join("cases.json"), serde_json::to_vec_pretty(&listed)?)?;
    println!(
        "{} cases of {} bake groups; kinds {kinds:?}",
        listed.len(),
        bake.groups.len()
    );
    Ok(())
}

/// The game's Oodle library (version 6), which decompresses its archives.
#[allow(unsafe_code)]
fn load_oodle(path: &Path) -> Result<Oodle, BoxError> {
    // SAFETY: the library shipped with the game, named by the caller.
    Ok(unsafe { Oodle::load(path, Version::V6) }?)
}
