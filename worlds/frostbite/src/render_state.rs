//! A permutation's fixed-function state ([`FixedState`]) as the shader stack's [`RenderState`].

use frostbite_formats::permutation::{BlendOp, Compare, Cull, Fill, FixedState, StencilFace, StencilOp};
use frostbite_formats::{Result, bad};
use iridia_interface as iri;
use iridia_interface::{
    Blend, BlendEquation, BlendFactor, DepthBias, DepthState, RenderState, Stencil, TargetState,
};

/// Blend factors in the game's `RenderBlendMode` order, as far as the interface names them (codes 10 to 16: saturate,
/// constant and second-source factors, have no name there).
const FACTORS: [BlendFactor; 10] = [
    BlendFactor::Zero,
    BlendFactor::One,
    BlendFactor::SrcColour,
    BlendFactor::OneMinusSrcColour,
    BlendFactor::SrcAlpha,
    BlendFactor::OneMinusSrcAlpha,
    BlendFactor::DstColour,
    BlendFactor::OneMinusDstColour,
    BlendFactor::DstAlpha,
    BlendFactor::OneMinusDstAlpha,
];

fn compare(c: Compare) -> iri::Compare {
    match c {
        Compare::Never => iri::Compare::Never,
        Compare::Less => iri::Compare::Less,
        Compare::Equal => iri::Compare::Equal,
        Compare::LessEqual => iri::Compare::LessEqual,
        Compare::Greater => iri::Compare::Greater,
        Compare::NotEqual => iri::Compare::NotEqual,
        Compare::GreaterEqual => iri::Compare::GreaterEqual,
        Compare::Always => iri::Compare::Always,
    }
}

fn stencil_op(op: StencilOp) -> iri::StencilOp {
    match op {
        StencilOp::Keep => iri::StencilOp::Keep,
        StencilOp::Zero => iri::StencilOp::Zero,
        StencilOp::Replace => iri::StencilOp::Replace,
        StencilOp::IncrementSaturate => iri::StencilOp::IncrementClamp,
        StencilOp::DecrementSaturate => iri::StencilOp::DecrementClamp,
        StencilOp::Invert => iri::StencilOp::Invert,
        StencilOp::Increment => iri::StencilOp::IncrementWrap,
        StencilOp::Decrement => iri::StencilOp::DecrementWrap,
    }
}

fn blend_op(op: BlendOp) -> iri::BlendOp {
    match op {
        BlendOp::Add => iri::BlendOp::Add,
        BlendOp::Subtract => iri::BlendOp::Subtract,
        BlendOp::ReverseSubtract => iri::BlendOp::ReverseSubtract,
        BlendOp::Min => iri::BlendOp::Min,
        BlendOp::Max => iri::BlendOp::Max,
    }
}

fn face(f: StencilFace) -> iri::StencilFace {
    iri::StencilFace {
        fail: stencil_op(f.fail),
        depth_fail: stencil_op(f.depth_fail),
        pass: stencil_op(f.pass),
        compare: compare(f.compare),
    }
}

/// The state as the interface's [`RenderState`], as D3D12 receives it. Checked against the game itself: the 24,218
/// pipeline descriptions `Skate.exe` handed D3D12 during a boot (captured at `D3D12Core`'s pipeline creation)
/// have every depth-bias clamp 0 and the decal row's colour blend operation `Add`, neither of which these bytes
/// state; so the clamp is 0 and a blended target's colour operation `Add`. A disabled depth test is D3D12's: no
/// test and no write. The stencil reference is set per draw (none stated). Refused: a blend factor the interface does
/// not name (codes 10 to 16) on a blended target.
pub fn render_state(state: &FixedState) -> Result<RenderState> {
    const W: &str = "fixed state";
    let factor = |code: u8| -> Result<BlendFactor> {
        FACTORS
            .get(usize::from(code))
            .copied()
            .ok_or_else(|| bad(W, 0, format!("blend mode {code} has no core factor")))
    };
    let mut targets = [None; 8];
    for (slot, t) in targets.iter_mut().zip(&state.targets) {
        let blend = if t.blend {
            Some(Blend {
                colour: BlendEquation {
                    source: factor(t.source)?,
                    destination: factor(t.destination)?,
                    op: iri::BlendOp::Add,
                },
                alpha: BlendEquation {
                    source: factor(t.source_alpha)?,
                    destination: factor(t.destination_alpha)?,
                    op: blend_op(t.alpha_op),
                },
            })
        } else {
            None
        };
        *slot = Some(TargetState {
            blend,
            write_mask: t.write_mask & 0xf,
        });
    }
    Ok(RenderState {
        blend: targets[0].and_then(|t| t.blend),
        depth: Some(if state.depth_test {
            DepthState {
                test: compare(state.depth_compare),
                write: state.depth_write,
            }
        } else {
            DepthState {
                test: iri::Compare::Always,
                write: false,
            }
        }),
        cull: Some(match state.cull {
            Cull::None => iri::Cull::None,
            Cull::Front => iri::Cull::Front,
            Cull::Back => iri::Cull::Back,
        }),
        alpha_test: None,
        targets,
        stencil: state.stencil_test.then_some(Stencil {
            read_mask: state.stencil_read_mask,
            write_mask: state.stencil_write_mask,
            reference: None,
            front: face(state.front),
            back: face(state.back),
        }),
        depth_bias: Some(DepthBias {
            constant: f32::from(state.depth_bias),
            slope_scaled: state.slope_scaled_depth_bias,
            clamp: 0.0,
        }),
        fill: Some(match state.fill {
            Fill::Solid => iri::Fill::Solid,
            Fill::Wireframe => iri::Fill::Wireframe,
        }),
        front_counter_clockwise: Some(state.front_counter_clockwise),
    })
}
