//! Elden Ring's SpeedTree models against the game's own SpeedTree vertex programs. Every SpeedTree FLVER of the asset
//! and map binders is read; each mesh's material's G-buffer vertex program (`<shader>_Gbuf.vpo` of the bundle its
//! material binary names, from the game's shader binders) is lifted and run by the executor on every vertex, and the
//! decoded geometry must be what the program outputs, in the world frame (z reversed, as an ordinary model's):
//! - the position: `SV_Position` (exact), with the view-projection the identity and the camera at the origin;
//! - UV0: `TEXCOORD6.xy` (exact), with the texture scroll the identity;
//! - the normal: `TEXCOORD0.xyz`, normalized (within 1e-6);
//! - the tangent: of the ordinary programs' convention, `w (TEXCOORD3.xyz × TEXCOORD0.xyz)` with `w` its `TEXCOORD3.w`
//!   ([`fromsoft_formats::flver::speedtree_layout`]), normalized (within 1e-6).
//!
//! The per-instance stream the game supplies is an identity instance: `POSITION` (0, 0, 0, 1) (no translation, scale
//! 1), `TEXCOORD1` (1, 0, 0) and `TEXCOORD0` (0, 1, 0) the basis vectors x and y (the programs take the third as their
//! cross product), `TEXCOORD0.w` 1 the level-of-detail blend at the finest level (the programs lerp from the stored
//! coarser position toward it). The wind is off: `VC_SpeedTreeWind`'s fourth row's `y` is 1, which every program tests
//! to leave the position as the basis gives it; every other constant is zero. A stored member is bound as
//! `TEXCOORD<its index>`.
//!
//! Each mesh's normal and tangent are the members its program reads as them, as `lower-fromsoft-geometry` takes them
//! with `speedtree-frame=program` ([`program_frame`], [`Model::set_speedtree_frame`]); the meshes where those are not
//! its layout's pair are listed. Billboard cards are the `billboard` material class, and their decode is refused.
use fromsoft_formats::flver::{Model, SpeedTreeLayout, is_speedtree, speedtree_layout};
use fromsoft_formats::{Binder, Game, NameDictionary, dcx::Decoder};
use iridia_exec::{Backend, Element, Executable, ImageSample, Resources, Runner};
use iridia_interface::{ResourceKind, ShaderFormat};
use primase_fromsoft::geometry::tangent_to_scene;
use primase_fromsoft::materials::{self, Definition, SkipClass};
use primase_fromsoft::sekiro::passes::geometry::program_frame;
use primase_fromsoft::sekiro::source::{ShaderBundle, ShaderProgram, ShaderStage};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;
#[allow(unsafe_code)]
mod native {
    pub fn load(
        path: primase_testdata::OodleLibrary,
    ) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_foundations::compress::oodle::Oodle::open(path.path(), primase_foundations::compress::oodle::Version::V6) }
    }
}

/// A vertex program reads no texture.
struct NoImages;
impl Backend for NoImages {
    fn sample(&mut self, _: ImageSample<Element>) -> std::result::Result<[f32; 4], String> {
        Err("a SpeedTree vertex program samples no texture".into())
    }
}

/// A lifted vertex program with the constants the module states, and the inputs it reads as the normal and tangent.
struct Prepared {
    frame: [u32; 2],
    exe: Executable,
    inputs: Vec<(String, u32, u8)>,
    outputs: BTreeMap<(String, u32), usize>,
    resources: Resources,
}

/// Shader `shader`'s G-buffer vertex program `name`, of bytes `code`.
fn prepare(shader: &str, name: &str, code: &[u8]) -> Result<Prepared> {
    let frame = program_frame(&ShaderBundle {
        name: shader.to_owned(),
        game: Game::EldenRing,
        programs: vec![ShaderProgram {
            name: name.to_owned(),
            stage: ShaderStage::Vertex,
            format: ShaderFormat::Dxil,
            code: code.into(),
        }],
        parameters: None,
    })?;
    let program = iridia_dxil::lift(code).map_err(|e| e.reason().to_string())?;
    let exe = Executable::new(&program)?;
    let mut resources = Resources::default();
    for r in &program.interface.resources {
        match &r.kind {
            ResourceKind::ConstantBuffer { layout } => {
                let mut rows = vec![[0u32; 4]; (layout.size as usize).div_ceil(16)];
                let mut set = |name: &str, row: usize, lane: usize| {
                    if let Some(c) = layout.constants.iter().find(|c| c.name == name) {
                        rows[c.offset as usize / 16 + row][c.offset as usize % 16 / 4 + lane] =
                            1f32.to_bits();
                    }
                };
                for i in 0..4 {
                    set("VC_MatrixViewProj", i, i);
                }
                set("VC_SpeedTreeWind", 3, 1);
                set("VC_TexScroll_0", 0, 0);
                set("VC_TexScroll_0", 0, 1);
                resources.constants.insert(r.id, rows);
            }
            // the instance index buffer: instance 0's entry
            ResourceKind::Buffer(_) => {
                resources.buffers.insert(r.id, vec![0; 4]);
            }
            other => return Err(format!("a resource of kind {other:?}").into()),
        }
    }
    let inputs = program
        .interface
        .inputs
        .iter()
        .map(|i| (i.label.name.clone(), i.label.index, i.value.width))
        .collect();
    let outputs = program
        .interface
        .outputs
        .iter()
        .enumerate()
        .map(|(k, o)| ((o.label.name.clone(), o.label.index), k))
        .collect();
    Ok(Prepared {
        frame,
        exe,
        inputs,
        outputs,
        resources,
    })
}

/// What the program outputs for each vertex of mesh `mesh`: position, normal, SpeedTree tangent and UV, as floats.
fn run(p: &Prepared, model: &Model, mesh: usize) -> Result<Vec<[[f32; 4]; 4]>> {
    let m = &model.meshes[mesh];
    let stored: Vec<_> = model.layouts[m.layouts[0]].iter().filter(|x| x.stored).collect();
    let at = |name: &str, index: u32| {
        p.outputs
            .get(&(name.to_owned(), index))
            .copied()
            .ok_or_else(|| format!("no output {name}{index}"))
    };
    let roles = [
        at("SV_Position", 0)?,
        at("TEXCOORD", 0)?,
        at("TEXCOORD", 3)?,
        at("TEXCOORD", 6)?,
    ];
    let bound = p.exe.bind_resources(&p.resources)?;
    let mut backend = NoImages;
    let mut runner = Runner::<1>::new(&p.exe, &bound, &mut backend, None);
    let mut out = Vec::with_capacity(m.vertices.len());
    for v in &m.vertices {
        let mut args = Vec::with_capacity(p.inputs.len());
        for (name, index, width) in &p.inputs {
            let lanes: [f32; 4] = match (name.as_str(), *index) {
                ("POSITION", 0) => [0., 0., 0., 1.],
                ("TEXCOORD", 0) => [0., 1., 0., 1.],
                ("TEXCOORD", 1) => [1., 0., 0., 0.],
                ("SV_InstanceID", 0) => [0.; 4],
                ("TEXCOORD", k) => {
                    let s = stored
                        .iter()
                        .position(|x| x.semantic == 5 && x.index == k)
                        .ok_or_else(|| {
                            format!("the program reads TEXCOORD{k}, which the layout does not store")
                        })?;
                    v.speedtree[s]
                }
                (n, k) => return Err(format!("input {n}{k}").into()),
            };
            let mut bits = lanes.map(f32::to_bits);
            bits[*width as usize..].fill(0);
            args.push(bits);
        }
        let [o] = runner.run([&args])?;
        out.push(roles.map(|k| o.values[k].map(f32::from_bits)));
    }
    Ok(out)
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|x| x / l)
}
fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= 1e-6)
}

#[derive(Default)]
struct Counts {
    models: usize,
    meshes: usize,
    vertices: usize,
    billboards: usize,
    /// Meshes whose program reads another normal and tangent than its layout's pair: `<model> mesh <n> (<shader>)`.
    reframed: Vec<String>,
}

#[test]
fn speedtree_models_are_what_their_programs_draw() -> Result<()> {
    let (Some(archive), Some(dll), Some(names)) = (
        common::archive(Game::EldenRing)?,
        primase_testdata::oodle(primase_testdata::Tool::Oodle6),
        primase_testdata::oracle(
            primase_testdata::OracleSet::Uxm,
            primase_testdata::Game::Elden,
            "dictionary.txt",
        ),
    ) else {
        return Ok(());
    };
    let game = Game::EldenRing;
    let oodle = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    // material binaries by stem, their shader's name, and the shaders' bundles by name
    let mut shaders = BTreeMap::new();
    for path in game.material_binders() {
        for f in Binder::read(&archive.read(path, &decoder)?, &decoder)?.files {
            let name = f.name.as_deref().unwrap_or("");
            if name.to_ascii_lowercase().ends_with(".matbin") {
                let m = fromsoft_formats::matbin::Material::read(&decoder.decode_if_dcx(&f.bytes)?)?;
                shaders.insert(fromsoft_formats::names::stem(name), m);
            }
        }
    }
    let mut bundles = BTreeMap::new();
    for path in game.shader_binders() {
        for f in Binder::read(&archive.read(path, &decoder)?, &decoder)?.files {
            let name = f
                .name
                .as_deref()
                .unwrap_or("")
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("");
            if let Some(shader) = name.strip_suffix(".shaderbdle")
                && shader.starts_with("SpeedTree_")
            {
                bundles.entry(shader.to_owned()).or_insert(f.bytes);
            }
        }
    }
    let mut programs: BTreeMap<String, Prepared> = BTreeMap::new();
    let names = NameDictionary::read_for(names, game)?;
    let mut paths: Vec<&String> = names
        .names
        .values()
        .flatten()
        .filter(|p| p.ends_with(".geombnd.dcx") || p.ends_with(".mapbnd.dcx"))
        .collect();
    paths.sort();
    paths.dedup();
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    for path in paths {
        let Ok(bytes) = archive.read(path, &decoder) else {
            continue;
        };
        for f in Binder::read(&bytes, &decoder)?.files {
            let data = decoder.decode_if_dcx(&f.bytes)?;
            if !is_speedtree(&data) {
                continue;
            }
            let id = format!("{path}:{}", f.name.as_deref().unwrap_or(""));
            let mut model = Model::read(&data).map_err(|e| format!("{id}: {e}"))?;
            counts.models += 1;
            for index in 0..model.meshes.len() {
                let mesh = &model.meshes[index];
                let source = &model.materials[mesh.material];
                let definition = shaders
                    .get(&fromsoft_formats::names::stem(&source.mtd))
                    .ok_or_else(|| format!("{id}: no material binary {}", source.mtd))?;
                let shader = definition.shader.rsplit(['/', '\\']).next().unwrap_or("");
                let shader = shader.rsplit_once('.').map_or(shader, |s| s.0).to_owned();
                let resolved = materials::resolve_definition(
                    source,
                    Some(&Definition::Matbin(definition.clone())),
                    game,
                    |_| false,
                );
                let billboard =
                    speedtree_layout(&model.layouts[mesh.layouts[0]])? == SpeedTreeLayout::Billboard;
                if billboard {
                    // the billboard program's material, of the class left out, and no geometry of its own
                    assert_eq!(shader, "SpeedTree_billboard", "{id} mesh {index}");
                    assert_eq!(resolved.skip, Some(SkipClass::Billboard), "{id} mesh {index}");
                    assert!(
                        primase_fromsoft::geometry::decode(&model, index, 0).is_err(),
                        "{id} mesh {index}"
                    );
                    counts.billboards += 1;
                    continue;
                }
                assert_eq!(resolved.skip, None, "{id} mesh {index}");
                if !programs.contains_key(&shader) {
                    let bundle = bundles
                        .get(&shader)
                        .ok_or_else(|| format!("{id}: no bundle {shader}"))?;
                    let wanted = format!("{shader}_Gbuf.vpo");
                    let code = Binder::read(bundle, &decoder)?
                        .files
                        .into_iter()
                        .find(|f| f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(&wanted))
                        .ok_or_else(|| format!("{shader}: no {wanted}"))?
                        .bytes;
                    programs.insert(
                        shader.clone(),
                        prepare(&shader, &wanted, &code).map_err(|e| format!("{wanted}: {e}"))?,
                    );
                }
                // the normal and tangent the program reads, as speedtree-frame=program gives them
                let frame = programs[&shader].frame;
                if model.speedtree_frame(index)? != Some(frame) {
                    model.set_speedtree_frame(index, frame)?;
                    counts.reframed.push(format!("{id} mesh {index} ({shader})"));
                }
                let mesh = &model.meshes[index];
                let outputs =
                    run(&programs[&shader], &model, index).map_err(|e| format!("{id} mesh {index}: {e}"))?;
                for face_set in 0..mesh.faces.len() {
                    let Some(part) = primase_fromsoft::geometry::decode(&model, index, face_set)? else {
                        continue;
                    };
                    for (j, &k) in part.source_vertices.iter().enumerate() {
                        let [p, n, t, uv] = outputs[k as usize];
                        let what = |role: &str| format!("{id} mesh {index} vertex {k}: {role}");
                        if part.mesh.positions[j] != [p[0], p[1], -p[2]] || p[3] != 1. {
                            failures.push(what("position"));
                        }
                        if part.mesh.uv[j] != [uv[0], uv[1]] {
                            failures.push(what("UV"));
                        }
                        if !near(part.mesh.normals[j], unit([n[0], n[1], -n[2]])) {
                            failures.push(what("normal"));
                        }
                        let w = t[3];
                        let tangent = [
                            w * (t[1] * n[2] - t[2] * n[1]),
                            w * (t[2] * n[0] - t[0] * n[2]),
                            w * (t[0] * n[1] - t[1] * n[0]),
                            w,
                        ];
                        let want = tangent_to_scene(tangent);
                        let got = part.tangents[j];
                        if !near([got[0], got[1], got[2]], [want[0], want[1], want[2]]) || got[3] != want[3] {
                            failures.push(what("tangent"));
                        }
                    }
                }
                counts.meshes += 1;
                counts.vertices += mesh.vertices.len();
            }
        }
    }
    println!(
        "SpeedTree models {}: meshes run {} ({} vertices, {} programs), billboard meshes {}, meshes whose program \
         reads another normal and tangent than its layout's pair {}, failures {}",
        counts.models,
        counts.meshes,
        counts.vertices,
        programs.len(),
        counts.billboards,
        counts.reframed.len(),
        failures.len()
    );
    for r in &counts.reframed {
        println!("  program's own frame: {r}");
    }
    for f in failures.iter().take(40) {
        println!("FAIL {f}");
    }
    assert!(counts.models > 0, "no SpeedTree model found");
    assert!(failures.is_empty());
    Ok(())
}
