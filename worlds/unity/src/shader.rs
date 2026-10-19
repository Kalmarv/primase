//! A Shader's render state as stored (`m_ParsedForm`): its subshaders, their passes and the passes' render-state
//! values, with the `BlendMode` values they use. The reader decodes them ([`crate::material::ShaderData`]),
//! `unity.material` carries them ([`crate::components::UnityMaterial::shader_state`]) and the passes choose by them.

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

    /// A pass of `kind` with `LightMode` `light_mode` blending `src` `dst`, writing depth, culling back faces.
    pub fn pass(kind: PassKind, light_mode: Option<&str>, src: ShaderValue, dst: ShaderValue) -> ShaderPass {
        ShaderPass {
            kind,
            light_mode: light_mode.map(str::to_owned),
            src_blend: src,
            dst_blend: dst,
            zwrite: value(1.0, None),
            cull: value(2.0, None),
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
