//! A Shader's render state as stored (`m_ParsedForm`): its subshaders, their passes and the passes' render-state
//! values, with the `BlendMode` values they use. The reader decodes them ([`crate::material::ShaderData`]),
//! `unity.material` carries them ([`crate::components::UnityMaterial::shader_state`]) and the passes choose by them;
//! [`procedural`] says which shaders build their geometry from component data.

/// Whether a shader builds its geometry from component data, drawing a placeholder mesh: Shapes (a widely used
/// package's line, disc, triangle and sphere shaders).
pub fn procedural(shader: &str) -> bool {
    shader.starts_with("Shapes/")
}

/// Unity's `BlendMode` values (`UnityEngine.Rendering.BlendMode`) the lowering names.
pub mod blend_mode {
    pub const ZERO: u8 = 0;
    pub const ONE: u8 = 1;
    pub const DST_COLOR: u8 = 2;
    pub const SRC_COLOR: u8 = 3;
    pub const SRC_ALPHA: u8 = 5;
    pub const ONE_MINUS_SRC_COLOR: u8 = 6;
    pub const ONE_MINUS_SRC_ALPHA: u8 = 10;
    /// The last value, `OneMinusSrcAlpha`.
    pub const LAST: u8 = 10;
}

/// A render-state value of a shader pass: the value the shader declares, and the material property it is bound to
/// (`Blend [_SrcBlend] ...`), which then gives the value.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderValue {
    pub value: f32,
    pub property: Option<String>,
}

/// What a pass is (`m_Type`): one with its own state and programs, a `UsePass` naming another shader's pass
/// (`m_UseName`, its state not here), or a `GrabPass` copying the screen to a texture (it draws nothing).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassKind {
    Normal,
    Use(String),
    Grab,
}

/// One render target's blend and write mask as the pass states them (`rtBlend0` to `rtBlend7`): Unity's `BlendMode`
/// factors and `BlendOp` operations, and the colour mask (bit 0 red to bit 3 alpha).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderTargetBlend {
    pub src: ShaderValue,
    pub dst: ShaderValue,
    pub src_alpha: ShaderValue,
    pub dst_alpha: ShaderValue,
    pub op: ShaderValue,
    pub op_alpha: ShaderValue,
    pub mask: ShaderValue,
}

/// A stencil face's operations (`SerializedStencilOp`): Unity's `StencilOp` for pass, fail and depth-fail, and its
/// `CompareFunction` (0 disabled, 1 never, 2 less, 3 equal, 4 less or equal, 5 greater, 6 not equal, 7 greater or
/// equal, 8 always).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderStencilOp {
    pub pass: ShaderValue,
    pub fail: ShaderValue,
    pub z_fail: ShaderValue,
    pub comp: ShaderValue,
}

/// The rest of a pass's render state as stored (`m_State`): every render target's blend, the depth test and offset and
/// the stencil.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PassState {
    /// `rtBlend0` to `rtBlend7`; the first is the pass's `src_blend` and `dst_blend`.
    pub targets: Vec<ShaderTargetBlend>,
    /// `rtSeparateBlend`: whether targets 1 to 7 state their own blend (else they take target 0's).
    pub separate_blend: bool,
    /// `zTest`, a `CompareFunction`.
    pub z_test: ShaderValue,
    /// `Offset factor, units`.
    pub offset_factor: ShaderValue,
    pub offset_units: ShaderValue,
    /// `stencilRef`, `stencilReadMask`, `stencilWriteMask`.
    pub stencil_ref: ShaderValue,
    pub stencil_read_mask: ShaderValue,
    pub stencil_write_mask: ShaderValue,
    /// `stencilOp`, `stencilOpFront`, `stencilOpBack`.
    pub stencil_op: ShaderStencilOp,
    pub stencil_op_front: ShaderStencilOp,
    pub stencil_op_back: ShaderStencilOp,
}

/// A shader pass's kind and render state: its `LightMode` tag as stored (Unity upper-cases some: `FORWARDBASE`), the
/// first render target's blend factors (Unity's `BlendMode`: 0 Zero, 1 One, 5 SrcAlpha, 10 OneMinusSrcAlpha, ...),
/// depth writes and culling (0 off, 1 front, 2 back).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderPass {
    pub kind: PassKind,
    pub light_mode: Option<String>,
    pub src_blend: ShaderValue,
    pub dst_blend: ShaderValue,
    pub zwrite: ShaderValue,
    pub cull: ShaderValue,
    pub state: PassState,
}

/// A subshader: its tags as stored (Unity upper-cases some keys: `QUEUE`) and its passes in order.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SubShader {
    pub tags: Vec<(String, String)>,
    pub passes: Vec<ShaderPass>,
}

/// Hand-written shader state for tests.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

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
}
