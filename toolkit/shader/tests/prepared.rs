#![allow(clippy::unwrap_used)]
//! Expected values come from evaluate.hlsl, not a second evaluation implementation.
use primase_shader::{
    Constants, Grid, Input, Invocation, Layout, PixelShader, PreparedProgram, PreparedProgramRef, Texture,
};
use std::collections::BTreeMap;

fn verify<S: PixelShader>(shader: S) {
    let mut constants = Constants::new(&Layout::of(&shader).unwrap());
    constants.set("tint", &[2., 4., 0.5, 1.]).unwrap();
    constants.set("tiling", &[2., 4.]).unwrap();
    constants.set("strength", &[2.]).unwrap();
    let mut prepared = PreparedProgram::new(
        shader,
        constants,
        BTreeMap::from([(3, Texture::Constant([0.25, 0.5, 0.75, 1.]))]),
    );
    assert_eq!(
        prepared
            .constants()
            .buffers()
            .map(|(slot, rows)| (slot, rows.len()))
            .collect::<Vec<_>>(),
        [(1, 2)]
    );
    assert!(matches!(
        prepared.textures()[&3],
        Texture::Constant([0.25, 0.5, 0.75, 1.])
    ));
    let inputs = BTreeMap::from([
        (("TEXCOORD".into(), 0), Input::Constant([0.25, 0.5, 0., 0.])),
        (("TEXCOORD".into(), 1), Input::Constant([1.5, 0.5, 0., 0.])),
        (("TEXCOORD".into(), 2), Input::Constant([0.75, 0., 0., 0.])),
    ]);
    let raw = prepared
        .as_ref()
        .evaluate_raw(
            &inputs,
            Grid {
                size: 1,
                period: [1.; 2],
            },
            &[(0, 15), (1, 15), (2, 15)],
            Invocation::Scalar,
        )
        .unwrap();
    assert_eq!(raw.pixels[0][0], ([0.5f32, 2., 0.375, 1.].map(f32::to_bits), 15));
    assert_eq!(raw.pixels[1][0].1, 0);
    assert_eq!(raw.pixels[2][0], ([3f32, 1., 0.75, 1.].map(f32::to_bits), 15));
    assert_eq!(raw.discarded, [false]);
    // Draw constants can change while shader and image ownership remain unchanged.
    prepared.constants_mut().set("tint", &[1.; 4]).unwrap();
    let updated = prepared
        .as_ref()
        .evaluate(
            &inputs,
            Grid {
                size: 1,
                period: [1.; 2],
            },
            &[(0, 15)],
        )
        .unwrap();
    assert_eq!(updated.targets[0].as_ref().unwrap()[0], [0.25, 0.5, 0.75, 1.]);
    let (shader, constants, textures) = prepared.into_parts();
    let borrowed = PreparedProgramRef::new(&shader, &constants, &textures);
    assert!(std::ptr::eq(borrowed.constants, &constants));
    let missing = PreparedProgramRef::new(&shader, &constants, &BTreeMap::new())
        .evaluate(
            &inputs,
            Grid {
                size: 1,
                period: [1.; 2],
            },
            &[(0, 15)],
        )
        .unwrap_err();
    assert!(missing.detail.contains("t3"));
}

#[test]
fn shared_contract_evaluates_dxbc_and_dxil_without_game_policy() {
    verify(primase_dxbc::Shader::parse(include_bytes!("fixtures/evaluate.dxbc")).unwrap());
    verify(primase_dxil::Shader::parse(include_bytes!("fixtures/evaluate.dxil")).unwrap());
}

#[derive(Debug)]
struct Discard(primase_shader::Reflection);
impl PixelShader for Discard {
    fn reflection(&self) -> &primase_shader::Reflection {
        &self.0
    }
    fn input_count(&self) -> usize {
        0
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &[]
    }
    fn textures(&self) -> &[u32] {
        &[]
    }
    fn samplers(&self) -> &[u32] {
        &[]
    }
    fn execute(
        &self,
        _: &[[u32; 4]],
        _: &[(u32, &[[u32; 4]])],
        _: &mut dyn primase_shader::Sampler,
    ) -> Result<primase_shader::Execution, primase_shader::Error> {
        Ok(primase_shader::Execution {
            outputs: vec![[1, 2, 3, 4]],
            output_masks: vec![7],
            discarded: true,
            instructions: 1,
            samples: 0,
        })
    }
    fn execute_quad(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn primase_shader::Sampler,
    ) -> Result<[primase_shader::Execution; 4], primase_shader::Error> {
        Ok([
            self.execute(inputs[0], buffers, sampler)?,
            self.execute(inputs[1], buffers, sampler)?,
            self.execute(inputs[2], buffers, sampler)?,
            self.execute(inputs[3], buffers, sampler)?,
        ])
    }
}

#[test]
fn invocation_and_raw_grid_preserve_discard_and_written_masks() {
    let shader = Discard(primase_shader::Reflection {
        buffers: vec![],
        resources: vec![],
        inputs: vec![],
        outputs: vec![],
    });
    let prepared = PreparedProgram::new(shader, Constants::new(&Layout::default()), BTreeMap::new());
    let execution = prepared.as_ref().execute(&[]).unwrap();
    assert!(execution.discarded);
    assert_eq!(execution.output_masks, [7]);
    let grid = Grid {
        size: 1,
        period: [1.; 2],
    };
    let raw = prepared
        .as_ref()
        .evaluate_raw(&BTreeMap::new(), grid, &[(0, 15)], Invocation::Scalar)
        .unwrap();
    assert_eq!(raw.discarded, [true]);
    assert_eq!(raw.pixels[0][0], ([1, 2, 3, 4], 7));
    assert!(
        prepared
            .as_ref()
            .evaluate(&BTreeMap::new(), grid, &[(0, 15)])
            .unwrap_err()
            .detail
            .contains("discarded")
    );
    let quad = prepared.as_ref().execute_quad([&[]; 4]).unwrap();
    assert!(
        quad.iter()
            .all(|pixel| pixel.discarded && pixel.output_masks == [7])
    );
    let raw_quad = prepared
        .as_ref()
        .evaluate_raw(
            &BTreeMap::new(),
            Grid {
                size: 2,
                period: [1.; 2],
            },
            &[(0, 15)],
            Invocation::Quads,
        )
        .unwrap();
    assert_eq!(raw_quad.discarded, [true; 4]);
    assert!(raw_quad.pixels[0].iter().all(|pixel| *pixel == ([1, 2, 3, 4], 7)));
    // A periodic colour bake cannot blend values from discarded fragments into visible neighbours.
    assert!(
        prepared
            .as_ref()
            .evaluate_periodic(&BTreeMap::new(), 2, 1, &[(0, 15)])
            .unwrap_err()
            .detail
            .contains("discarded")
    );
}
