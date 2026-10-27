//! Hand-written shader state for tests.

use unity_formats::shader::{
    PassKind, PassState, ShaderPass, ShaderStencilOp, ShaderTargetBlend, ShaderValue, SubShader,
};

/// A render-state value: the shader's own, or bound to a material property.
pub fn value(value: f32, property: Option<&str>) -> ShaderValue {
    ShaderValue {
        value,
        property: property.map(str::to_owned),
    }
}

/// Unity's defaults: every target blends One Zero with all channels written, a less-or-equal depth test, no offset,
/// a stencil that always passes and keeps (reference 0, masks 255).
pub fn default_state() -> PassState {
    let v = |x| value(x, None);
    let target = || ShaderTargetBlend {
        src: v(1.0),
        dst: v(0.0),
        src_alpha: v(1.0),
        dst_alpha: v(0.0),
        op: v(0.0),
        op_alpha: v(0.0),
        mask: v(15.0),
    };
    let op = || ShaderStencilOp {
        pass: v(0.0),
        fail: v(0.0),
        z_fail: v(0.0),
        comp: v(8.0),
    };
    PassState {
        targets: (0..8).map(|_| target()).collect(),
        separate_blend: false,
        z_test: v(4.0),
        offset_factor: v(0.0),
        offset_units: v(0.0),
        stencil_ref: v(0.0),
        stencil_read_mask: v(255.0),
        stencil_write_mask: v(255.0),
        stencil_op: op(),
        stencil_op_front: op(),
        stencil_op_back: op(),
    }
}

/// A pass of `kind` with `LightMode` `light_mode` blending `src` `dst`, writing depth, culling back faces.
pub fn pass(kind: PassKind, light_mode: Option<&str>, src: ShaderValue, dst: ShaderValue) -> ShaderPass {
    // the first target's alpha equation is the colour's
    let mut state = default_state();
    state.targets[0].src = src.clone();
    state.targets[0].src_alpha = src.clone();
    state.targets[0].dst = dst.clone();
    state.targets[0].dst_alpha = dst.clone();
    ShaderPass {
        kind,
        light_mode: light_mode.map(str::to_owned),
        src_blend: src,
        dst_blend: dst,
        zwrite: value(1.0, None),
        cull: value(2.0, None),
        state,
    }
}

/// A normal pass with its own blend factors.
pub fn own(light_mode: Option<&str>, src: f32, dst: f32) -> ShaderPass {
    pass(PassKind::Normal, light_mode, value(src, None), value(dst, None))
}

pub fn sub(tags: &[(&str, &str)], passes: Vec<ShaderPass>) -> SubShader {
    SubShader {
        tags: tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        passes,
    }
}

/// Built-in Standard's first subshader as stored: a base pass blending `[_SrcBlend] [_DstBlend]` (One One where
/// the material has neither), an additive per-light pass (`[_SrcBlend] One`), a shadow caster and a deferred pass.
pub fn standard() -> SubShader {
    let bound = |p| value(1.0, Some(p));
    sub(
        &[("RenderType", "Opaque")],
        vec![
            pass(
                PassKind::Normal,
                Some("FORWARDBASE"),
                bound("_SrcBlend"),
                bound("_DstBlend"),
            ),
            pass(
                PassKind::Normal,
                Some("FORWARDADD"),
                bound("_SrcBlend"),
                value(1.0, None),
            ),
            own(Some("SHADOWCASTER"), 1.0, 0.0),
            own(Some("DEFERRED"), 1.0, 0.0),
        ],
    )
}
