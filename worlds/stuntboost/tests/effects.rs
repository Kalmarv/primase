#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The install's compiled effects (`installs.stuntboost/Content/Effects`: XNB files of MonoGame's `EffectReader`, MGFX
//! version 12, OpenGL profile) as the documents they store.
//!
//! - E1: every effect is an XNB whose reader is the effect reader and whose effect reads to its end: OpenGL, shaders
//!   of GLSL text that is vertex or pixel as it says, every technique's passes naming shaders of the stage they are
//!   for, every sampler naming a parameter, every constant buffer parameter naming one, and every shader's GLSL
//!   declaring the samplers, the vertex attributes and the uniform array its tables list.

use std::path::{Path, PathBuf};

use primase_monogame::mgfx::{Effect, Profile, Stage};
use primase_monogame::xnb::Xnb;
use primase_testdata::{Game, install_file};

fn effects(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            effects(&path, out);
        } else if path.extension().is_some_and(|e| e == "xnb") {
            out.push(path);
        }
    }
}

#[test]
fn e1_every_effect_reads_to_its_shaders_and_passes() {
    let Some(root) = install_file(Game::Stuntboost, "Content/Effects") else {
        return;
    };
    let mut files = Vec::new();
    effects(&root, &mut files);
    assert!(!files.is_empty(), "no effects in {}", root.display());
    let (mut shaders, mut samplers, mut techniques) = (0usize, 0usize, 0usize);
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        let bytes = std::fs::read(path).unwrap();
        let xnb = Xnb::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let code = xnb.effect().unwrap_or_else(|e| panic!("{name}: {e}"));
        let effect = Effect::parse(code).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(effect.profile, Profile::OpenGl, "{name}");
        for shader in &effect.shaders {
            shaders += 1;
            let glsl =
                std::str::from_utf8(shader.code).unwrap_or_else(|e| panic!("{name}: GLSL is not UTF-8: {e}"));
            assert!(glsl.contains("void main()"), "{name}");
            let prefix = if shader.stage == Stage::Vertex {
                "vs_"
            } else {
                "ps_"
            };
            for sampler in &shader.samplers {
                samplers += 1;
                assert!(
                    glsl.contains(&format!("uniform sampler2D {};", sampler.name))
                        || glsl.contains(&format!("{};", sampler.name)),
                    "{name}: sampler {}",
                    sampler.name
                );
                assert!(
                    effect.parameters.get(usize::from(sampler.parameter)).is_some(),
                    "{name}: sampler {} parameter",
                    sampler.name
                );
            }
            for attribute in &shader.attributes {
                assert!(
                    glsl.contains(&format!("attribute vec4 {};", attribute.name)),
                    "{name}: {}",
                    attribute.name
                );
                assert!(attribute.name.starts_with(prefix), "{name}");
            }
            for &b in &shader.constant_buffers {
                let buffer = &effect.constant_buffers[usize::from(b)];
                assert_eq!(buffer.name, format!("{prefix}uniforms_vec4"), "{name}");
                assert!(glsl.contains(&format!("uniform vec4 {}[", buffer.name)), "{name}");
                for &(parameter, offset) in &buffer.parameters {
                    assert!((parameter as usize) < effect.parameters.len(), "{name}");
                    assert!(offset < buffer.size, "{name}");
                }
            }
        }
        for technique in &effect.techniques {
            techniques += 1;
            for pass in &technique.passes {
                let vertex = pass.vertex_shader.map(|i| effect.shaders[i].stage);
                let pixel = pass.pixel_shader.map(|i| effect.shaders[i].stage);
                assert_eq!(
                    (vertex, pixel),
                    (Some(Stage::Vertex), Some(Stage::Pixel)),
                    "{name}: {}",
                    technique.name
                );
            }
        }
    }
    println!(
        "{} effects, {shaders} shaders, {samplers} samplers, {techniques} techniques",
        files.len()
    );
    assert_eq!(files.len(), 18);
}
