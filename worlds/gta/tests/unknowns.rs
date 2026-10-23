//! What `resolve-gta-shaders` still leaves Unknown on a region of the map (Vespucci Beach by default; the debug key
//! `gta.region` `x0,y0,x1,y1`), for the technique `deferred_draw` (the G-buffer pass of an unskinned mesh): the materials left unbound by reason, and over the bound materials' stages the output lanes, the
//! vertex inputs, the constants (by constant buffer and name) and the texture slots with no role or no texture, the
//! render state and the samplers. The counts are printed and the totals asserted to add up; the numbers are what is
//! left for full GTA shader support.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::BTreeMap;

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{
    ConstantRole, InputRole, OutputRole, RenderState, Shader, ShaderBinding, Stage, TextureSource,
};
use primase_gta::components::{GtaEffectTechniques, GtaMaterialEffect, GtaShader, GtaShaderTable};
use primase_gta::passes::resolve_shaders::ResolveGtaShaders;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, debug, install, keys};

#[test]
fn what_is_left_unknown_on_a_region() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let region: Vec<f32> = debug("gta.region")
        .unwrap_or_else(|| "-1500,-1650,-1150,-1330".into())
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    let source = GtaSource::open(&dir, &keys).unwrap();
    let (read, _) = source
        .read([region[0], region[1], region[2], region[3]], 600.0)
        .unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    let materials = world.iter::<GtaShader>().count();
    source.add_shader_database(&mut world).unwrap();
    let options = Options::from([
        ("technique".to_owned(), "deferred_draw".to_owned()),
        ("unset-textures".to_owned(), "white".to_owned()),
    ]);
    let mut log = PassLog::default();
    ResolveGtaShaders::from_options(&options)
        .unwrap()
        .run(&mut world, &mut log)
        .unwrap();
    let bound = world.iter::<ShaderBinding>().count();
    println!("{materials} materials, {bound} bound");
    let unbound: u64 = log
        .counts
        .iter()
        .filter(|(k, _)| k.starts_with("materials left unbound"))
        .map(|(_, n)| n)
        .sum();
    println!("unbound: {unbound}");
    // by reason, without the effect's name
    let mut reasons: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    for (what, n) in log
        .counts
        .iter()
        .filter(|(k, _)| k.starts_with("materials left unbound"))
    {
        let (reason, _) = what.rsplit_once(" (").unwrap_or((what, ""));
        let e = reasons.entry(reason.to_owned()).or_default();
        e.0 += n;
        e.1 += 1;
    }
    for (reason, (n, effects)) in &reasons {
        println!("  {n:>6} materials in {effects} effects: {reason}");
    }
    assert_eq!(bound as u64 + unbound, materials as u64);
    // what the unbound materials' effects do have: techniques of the G-buffer pass or only forward ones
    let mut elsewhere: BTreeMap<String, usize> = BTreeMap::new();
    for (e, link) in world.iter::<GtaMaterialEffect>() {
        if world.has::<ShaderBinding>(e) {
            continue;
        }
        let key = match world.get::<GtaEffectTechniques>(link.effect) {
            None => "no technique table".to_owned(),
            Some(t) => {
                let mut deferred: Vec<&str> = t
                    .techniques
                    .iter()
                    .map(|t| t.name.as_str())
                    .filter(|n| n.starts_with("deferred"))
                    .collect();
                deferred.sort_unstable();
                if deferred.is_empty() {
                    "no deferred technique (forward and shadow techniques only)".to_owned()
                } else if deferred.contains(&"deferred_draw") {
                    "deferred_draw is tessellated or has several passes".to_owned()
                } else {
                    format!(
                        "deferred techniques without deferred_draw: {}",
                        deferred.join(", ")
                    )
                }
            }
        };
        *elsewhere.entry(key).or_default() += 1;
    }
    println!("what the unbound materials' effects do have:");
    for (k, n) in &elsewhere {
        println!("  {n:>6} {k}");
    }

    let mut outputs: BTreeMap<String, usize> = BTreeMap::new();
    let mut inputs: BTreeMap<String, usize> = BTreeMap::new();
    let mut constants: BTreeMap<(u32, String, String), usize> = BTreeMap::new();
    let mut textures: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
    let mut default_state = 0usize;
    let mut samplers_bound = 0usize;
    for (_, binding) in world.iter::<ShaderBinding>() {
        if binding.state == RenderState::default() {
            default_state += 1;
        }
        for stage in &binding.stages {
            let program = world.get::<Shader>(stage.shader).unwrap();
            let table = &world.get::<GtaShaderTable>(stage.shader).unwrap().0;
            let space_of = |id| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| r.id == id)
                    .and_then(|r| r.provenance.d3d)
            };
            for o in &stage.outputs {
                if matches!(o.role, OutputRole::Unknown(_)) {
                    *outputs
                        .entry(format!(
                            "{:?} {}{} lanes {:04b}",
                            stage.stage, o.label.name, o.label.index, o.lanes
                        ))
                        .or_default() += 1;
                }
            }
            for i in &stage.inputs {
                if let InputRole::Unknown { why } = &i.role {
                    *inputs
                        .entry(format!(
                            "{:?} {}{}: {why}",
                            stage.stage, i.label.name, i.label.index
                        ))
                        .or_default() += 1;
                }
            }
            for c in &stage.constants {
                if let ConstantRole::Unknown { why } = &c.role {
                    let d3d = space_of(c.resource);
                    let space = d3d.map_or(99, |d| d.space);
                    // the constant buffer's name in the program's table
                    let buffer = d3d
                        .and_then(|d| {
                            table.resources.iter().find(|r| {
                                u32::from(r.register) == d.register
                                    && u32::from(r.space) == d.space
                                    && r.constants > 0
                            })
                        })
                        .and_then(|r| table.name_of(r.name))
                        .unwrap_or("?")
                        .to_owned();
                    let why = if why.contains("stores no value") {
                        "material parameter the material stores no value for"
                    } else {
                        "engine constant"
                    };
                    *constants
                        .entry((
                            space,
                            format!("{buffer}.{}", c.name.clone().unwrap_or_default()),
                            why.to_owned(),
                        ))
                        .or_default() += 1;
                }
            }
            for t in &stage.textures {
                if t.sampler.is_some() {
                    samplers_bound += 1;
                }
                let kind = match &t.source {
                    TextureSource::Texture { .. } => continue,
                    TextureSource::Default { .. } => "default (white)",
                    TextureSource::Missing => "missing",
                };
                let d3d = space_of(t.resource);
                let name = d3d
                    .and_then(|d| {
                        table.resources.iter().find(|r| {
                            u32::from(r.register) == d.register
                                && u32::from(r.space) == d.space
                                && r.constants == 0
                        })
                    })
                    .and_then(|r| table.name_of(r.name))
                    .unwrap_or("?")
                    .to_owned();
                *textures
                    .entry((format!("{:?} {name}", stage.stage), kind))
                    .or_default() += 1;
            }
        }
    }
    println!(
        "render state left at the default: {default_state} of {bound}; samplers bound: {samplers_bound}"
    );
    println!("output lanes left Unknown (stage target lanes: bindings):");
    for (k, n) in &outputs {
        println!("  {n:>6} {k}");
    }
    println!("inputs left Unknown:");
    for (k, n) in &inputs {
        println!("  {n:>6} {k}");
    }
    // constants: by class, then the most common names
    let mut by_class: BTreeMap<(u32, String), (usize, usize)> = BTreeMap::new();
    for ((space, _, why), n) in &constants {
        let e = by_class.entry((*space, why.clone())).or_default();
        e.0 += 1;
        e.1 += n;
    }
    println!("constants left Unknown, by constant buffer space and kind (distinct names, bindings):");
    for ((space, why), (names, n)) in &by_class {
        println!("  space {space} {why}: {names} distinct names, {n} bindings");
    }
    let mut top: Vec<_> = constants.iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for ((space, name, _), n) in top.iter().take(12) {
        println!("    space {space} {name}: {n}");
    }
    println!("textures with no texture, by slot (stage, name) and kind:");
    for ((slot, kind), n) in &textures {
        println!("  {n:>6} {slot}: {kind}");
    }
    let _ = Stage::Pixel;
}
