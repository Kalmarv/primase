//! A Shader's compiled programs, attached to the world's materials: for every `unity.material`, the programs of its draw
//! pass as stored (`unity.programs`), their variants' D3D11 code and the shader's declared texture defaults. Reading the
//! programs from the player's files is `unity_formats::program`'s.

use unity_formats::Result;
use unity_formats::player::ObjRef;
use unity_formats::program::{
    D3D11_PROGRAM_TYPES, PassProgram, Segments, ShaderPrograms, Stage, dxbc, merge_parameters,
    parameter_blob, texture_defaults,
};

/// What [`read_programs`] attached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramsReport {
    /// Materials given their program.
    pub attached: usize,
    /// Materials of the shaders without one, by reason.
    pub left_out: std::collections::BTreeMap<String, usize>,
}

/// For every `unity.material` of `w` whose shader is one of `shaders`, the programs of its draw pass under
/// `pipeline` ([`UnityMaterial::draw_pass`]) as stored, each stage's (vertex, fragment, geometry, hull, domain): every
/// variant with its D3D11 code, and the shader's declared texture defaults, as `unity.programs` on the material's
/// entity (choosing a variant is the caller's, [`UnityPrograms::variant`]). A material without a shader object, draw
/// pass or D3D11 code is left out, counted by reason; so are variants whose code holds no DXBC container.
pub fn read_programs(
    player: &unity_formats::player::Player,
    w: &mut primase_core::World,
    shaders: &[&str],
    pipeline: &unity_formats::player::RenderPipeline,
) -> Result<ProgramsReport> {
    use crate::components::{UnityMaterial, UnityProgramVariant, UnityPrograms};
    let mut report = ProgramsReport::default();
    let mut read: std::collections::BTreeMap<(String, i64), (ShaderPrograms, Segments)> =
        std::collections::BTreeMap::new();
    // each (shader, subshader, pass)'s variants of every stage and the shader's texture defaults, made once
    type Built = (
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<(String, String)>,
    );
    let mut built: std::collections::BTreeMap<((String, i64), usize, usize), Built> =
        std::collections::BTreeMap::new();
    let materials: Vec<(primase_core::Entity, UnityMaterial)> = w
        .iter::<UnityMaterial>()
        .filter(|(_, m)| m.shader.as_deref().is_some_and(|s| shaders.contains(&s)))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    for (e, m) in materials {
        let mut leave = |why: &str| *report.left_out.entry(why.to_owned()).or_default() += 1;
        let data = unity_formats::material::UnityMaterialData::read(
            player,
            &ObjRef {
                file: m.material.file.clone(),
                path_id: m.material.path_id,
            },
        )?;
        let Some(shader) = data.shader else {
            leave("no shader object");
            continue;
        };
        // the draw pass's place in the shader's parsed form, which `shader_state` mirrors
        let place = m.subshader(pipeline).and_then(|sub| {
            let i = m.shader_state.iter().position(|s| std::ptr::eq(s, sub))?;
            let pass = m.draw_pass(pipeline)?;
            let j = sub.passes.iter().position(|p| std::ptr::eq(p, pass))?;
            Some((i, j))
        });
        let Some((i, j)) = place else {
            leave("no draw pass");
            continue;
        };
        let key = (shader.file.clone(), shader.path_id);
        if !read.contains_key(&key) {
            read.insert(
                key.clone(),
                (ShaderPrograms::read(player, &shader)?, Segments::default()),
            );
        }
        let (programs, segments) = read.get_mut(&key).expect("read above");
        let programs = &*programs;
        let Some(pass) = programs.passes.get(i).and_then(|sub| sub.get(j)) else {
            leave("no draw pass");
            continue;
        };
        if !programs.platforms.contains(&4) {
            leave("no D3D11 code");
            continue;
        }
        let built_key = (key.clone(), i, j);
        if !built.contains_key(&built_key) {
            let mut stored = |stage: Stage, program: &PassProgram| -> Result<Vec<UnityProgramVariant>> {
                let mut variants = Vec::new();
                for v in &program.variants {
                    // a pass stores the variants of every graphics API its shader was compiled for; this blob is D3D11's
                    if !D3D11_PROGRAM_TYPES.contains(&v.gpu_program_type) {
                        *report
                            .left_out
                            .entry("variants of another graphics API than D3D11".to_owned())
                            .or_default() += 1;
                        continue;
                    }
                    let code = programs.code_with(segments, 4, v.blob_index)?;
                    let Some(code) = dxbc(&code) else {
                        *report
                            .left_out
                            .entry(match stage {
                                Stage::Fragment => {
                                    "variants whose D3D11 code holds no DXBC container".to_owned()
                                }
                                _ => format!(
                                    "{} variants whose D3D11 code holds no DXBC container",
                                    format!("{stage:?}").to_lowercase()
                                ),
                            })
                            .or_default() += 1;
                        continue;
                    };
                    // a 2021.2 player stores a variant's own parameters in a blob entry beside the pass's common ones
                    let parameters = match v.parameter_blob {
                        Some(blob) => {
                            match programs
                                .entry_with(segments, 4, blob)
                                .and_then(|entry| parameter_blob(&entry))
                            {
                                Ok(own) => merge_parameters(&program.parameters, &own),
                                Err(_) => {
                                    *report
                                        .left_out
                                        .entry(
                                            "variants whose parameter blob is not read (a structured buffer of DOTS instancing): the pass's common parameters only"
                                                .to_owned(),
                                        )
                                        .or_default() += 1;
                                    program.parameters_of(v).clone()
                                }
                            }
                        }
                        None => program.parameters_of(v).clone(),
                    };
                    variants.push(UnityProgramVariant {
                        tier: v.group,
                        gpu_program_type: v.gpu_program_type,
                        keywords: v.keywords.clone(),
                        parameters,
                        code: std::sync::Arc::from(code),
                    });
                }
                Ok(variants)
            };
            let variants = stored(Stage::Fragment, &pass.fragment)?;
            let vertex = stored(Stage::Vertex, &pass.vertex)?;
            let geometry = stored(Stage::Geometry, &pass.geometry)?;
            let hull = stored(Stage::Hull, &pass.hull)?;
            let domain = stored(Stage::Domain, &pass.domain)?;
            let texture_defaults = texture_defaults(player, &shader)?;
            built.insert(
                built_key.clone(),
                (variants, vertex, geometry, hull, domain, texture_defaults),
            );
        }
        let (variants, vertex, geometry, hull, domain, texture_defaults) = built[&built_key].clone();
        w.insert(
            e,
            UnityPrograms {
                shader: programs.name.clone(),
                subshader: i,
                pass: j,
                texture_defaults,
                variants,
                vertex,
                geometry,
                hull,
                domain,
            },
        );
        report.attached += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use unity_formats::program::Parameters;

    /// The caller states the renderer's keywords and tier; the material adds the keywords the pass's variants use
    /// (a stale one no variant has is not compiled in); the keywords must match exactly, in the stated tier.
    #[test]
    fn a_variant_is_the_one_the_caller_states() {
        use crate::components::{UnityProgramVariant, UnityPrograms};
        let variant = |tier: usize, keywords: &[&str]| UnityProgramVariant {
            tier,
            gpu_program_type: 18,
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            parameters: Parameters::default(),
            code: std::sync::Arc::from(vec![tier as u8, keywords.len() as u8]),
        };
        let programs = UnityPrograms {
            shader: "Test".into(),
            subshader: 0,
            pass: 0,
            texture_defaults: Vec::new(),
            vertex: Vec::new(),
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
            variants: vec![
                variant(1, &["DIRECTIONAL"]),
                variant(0, &["DIRECTIONAL"]),
                variant(0, &["DIRECTIONAL", "SHADOWS_SCREEN"]),
                variant(0, &["_NORMALMAP", "DIRECTIONAL"]),
            ],
        };
        let s = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let chosen = |material: &[&str], global: &[&str], tier| {
            programs
                .variant(&s(material), &s(global), tier)
                .map(|v| v.code.to_vec())
        };
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 0), Some(vec![0, 1]));
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 1), Some(vec![1, 1]));
        assert_eq!(
            chosen(&["_NORMALMAP", "STALE"], &["DIRECTIONAL"], 0),
            Some(vec![0, 2])
        );
        assert_eq!(
            chosen(&[], &["DIRECTIONAL", "SHADOWS_SCREEN"], 0),
            Some(vec![0, 2])
        );
        assert_eq!(chosen(&[], &["DIRECTIONAL", "LIGHTMAP_ON"], 0), None);
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 2), None);
    }
}
