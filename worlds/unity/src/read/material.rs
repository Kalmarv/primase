//! Unity materials as stored: the shader's name and render state (decoded into [`crate::shader`]'s types), and the saved
//! properties (textures with their tiling and offset, floats, integers, colours), keywords, tags and render queue. What
//! they mean depends on the shader; lowering to `pbr.material` is [`crate::lower`].

use crate::read::player::{ObjRef, Player};
use crate::read::value::{Array, Value};
use crate::shader::{
    PassKind, PassState, ShaderPass, ShaderStencilOp, ShaderTargetBlend, ShaderValue, SubShader,
};
use crate::{Result, invalid};

/// A texture property: the texture (`None`: no texture set) and its tiling (`_ST`) with Unity's bottom-left origin.
#[derive(Clone, Debug, PartialEq)]
pub struct TexEnv {
    pub texture: Option<ObjRef>,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// A `Material` as stored, its properties in stored order.
#[derive(Clone, Debug, PartialEq)]
pub struct UnityMaterialData {
    pub name: String,
    pub shader: Option<ObjRef>,
    pub textures: Vec<(String, TexEnv)>,
    pub floats: Vec<(String, f32)>,
    pub ints: Vec<(String, i64)>,
    /// RGBA as stored (gamma-encoded for colour properties).
    pub colors: Vec<(String, [f32; 4])>,
    pub keywords: Vec<String>,
    pub tags: Vec<(String, String)>,
    /// `m_CustomRenderQueue` (-1: the shader's).
    pub render_queue: i64,
}

fn list<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match v.get(k) {
        Some(Value::Array(Array::Values(items))) => Ok(items),
        Some(Value::Array(a)) if a.is_empty() => Ok(&[]),
        None => Ok(&[]),
        _ => Err(invalid(format!("material: {k} is not a list"))),
    }
}

/// A list of `pair(string, T)` as (name, T).
fn pairs<'a, T>(v: &'a Value, k: &str, f: impl Fn(&'a Value) -> Option<T>) -> Result<Vec<(String, T)>> {
    list(v, k)?
        .iter()
        .map(|p| match p {
            Value::Pair(kv) => {
                let name =
                    kv.0.as_str()
                        .ok_or_else(|| invalid(format!("material: {k} key")))?;
                let value = f(&kv.1).ok_or_else(|| invalid(format!("material: {k} {name}")))?;
                Ok((name.to_owned(), value))
            }
            _ => Err(invalid(format!("material: {k} entry is not a pair"))),
        })
        .collect()
}

fn f2(v: &Value) -> Option<[f32; 2]> {
    Some([v.get("x")?.as_f32()?, v.get("y")?.as_f32()?])
}

fn strings(v: &Value, k: &str) -> Result<Vec<String>> {
    list(v, k)?
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("material: {k} entry")))
        })
        .collect()
}

/// A material's enabled keywords: `m_ValidKeywords` (Unity 2021.2 on; `m_InvalidKeywords`, those its shader does not
/// declare, are not enabled), else `m_ShaderKeywords`, one string separated by spaces (earlier versions).
fn keywords(m: &Value) -> Result<Vec<String>> {
    if m.get("m_ValidKeywords").is_some() {
        return strings(m, "m_ValidKeywords");
    }
    match m.get("m_ShaderKeywords") {
        None => Ok(Vec::new()),
        Some(v) => Ok(v
            .as_str()
            .ok_or_else(|| invalid("material: m_ShaderKeywords is not a string"))?
            .split_whitespace()
            .map(str::to_owned)
            .collect()),
    }
}

impl UnityMaterialData {
    /// Read the Material `r` of `player`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let m = player.decode(r)?;
        let props = m
            .get("m_SavedProperties")
            .ok_or_else(|| invalid("material: no m_SavedProperties"))?;
        let shader = match m.get("m_Shader") {
            Some(p) => player.resolve(&r.file, p)?,
            None => None,
        };
        let textures = list(props, "m_TexEnvs")?
            .iter()
            .map(|p| {
                let Value::Pair(kv) = p else {
                    return Err(invalid("material: m_TexEnvs entry is not a pair"));
                };
                let name = kv.0.as_str().ok_or_else(|| invalid("material: texture name"))?;
                let env = &kv.1;
                let texture = match env.get("m_Texture") {
                    Some(t) => player.resolve(&r.file, t)?,
                    None => None,
                };
                let ts = |k: &str| {
                    env.get(k)
                        .and_then(f2)
                        .ok_or_else(|| invalid(format!("material: {name} {k}")))
                };
                Ok((
                    name.to_owned(),
                    TexEnv {
                        texture,
                        scale: ts("m_Scale")?,
                        offset: ts("m_Offset")?,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        Ok(UnityMaterialData {
            name: m.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
            shader,
            textures,
            floats: pairs(props, "m_Floats", Value::as_f32)?,
            ints: pairs(props, "m_Ints", Value::as_i64)?,
            colors: pairs(props, "m_Colors", |c| {
                Some([
                    c.get("r")?.as_f32()?,
                    c.get("g")?.as_f32()?,
                    c.get("b")?.as_f32()?,
                    c.get("a")?.as_f32()?,
                ])
            })?,
            keywords: keywords(&m)?,
            tags: pairs(&m, "stringTagMap", |s| s.as_str().map(str::to_owned))?,
            render_queue: m.get("m_CustomRenderQueue").and_then(Value::as_i64).unwrap_or(-1),
        })
    }
}

/// A Shader's name and render state (`m_ParsedForm`); its compiled programs are not read.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderData {
    pub name: String,
    pub subshaders: Vec<SubShader>,
}

fn tag_list(v: Option<&Value>) -> Result<Vec<(String, String)>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let inner = v.get("tags").or_else(|| v.get("m_Tags")).unwrap_or(v);
    pairs_of(inner, "tag")
}

fn pairs_of(v: &Value, what: &str) -> Result<Vec<(String, String)>> {
    let items: &[Value] = match v {
        Value::Array(Array::Values(items)) => items,
        Value::Array(a) if a.is_empty() => &[],
        _ => return Err(invalid(format!("shader: {what}s are not a list"))),
    };
    items
        .iter()
        .map(|p| match p {
            Value::Pair(kv) => Ok((
                kv.0.as_str()
                    .ok_or_else(|| invalid(format!("shader: {what} key")))?
                    .to_owned(),
                kv.1.as_str()
                    .ok_or_else(|| invalid(format!("shader: {what} value")))?
                    .to_owned(),
            )),
            _ => Err(invalid(format!("shader: {what} entry is not a pair"))),
        })
        .collect()
}

fn shader_value(v: Option<&Value>, what: &str) -> Result<ShaderValue> {
    let v = v.ok_or_else(|| invalid(format!("shader pass: no {what}")))?;
    let value = v
        .get("val")
        .and_then(Value::as_f32)
        .ok_or_else(|| invalid(format!("shader pass: {what} without a value")))?;
    let property = match v.get("name").and_then(Value::as_str) {
        None | Some("") | Some("<noninit>") => None,
        Some(n) => Some(n.to_owned()),
    };
    Ok(ShaderValue { value, property })
}

/// A render target's blend (`rtBlend<n>`).
fn target_blend(rt: &Value) -> Result<ShaderTargetBlend> {
    Ok(ShaderTargetBlend {
        src: shader_value(rt.get("srcBlend"), "srcBlend")?,
        dst: shader_value(rt.get("destBlend"), "destBlend")?,
        src_alpha: shader_value(rt.get("srcBlendAlpha"), "srcBlendAlpha")?,
        dst_alpha: shader_value(rt.get("destBlendAlpha"), "destBlendAlpha")?,
        op: shader_value(rt.get("blendOp"), "blendOp")?,
        op_alpha: shader_value(rt.get("blendOpAlpha"), "blendOpAlpha")?,
        mask: shader_value(rt.get("colMask"), "colMask")?,
    })
}

/// A stencil face's operations (`stencilOp`, `stencilOpFront`, `stencilOpBack`).
fn stencil_op(st: &Value, what: &str) -> Result<ShaderStencilOp> {
    let op = st
        .get(what)
        .ok_or_else(|| invalid(format!("shader pass: no {what}")))?;
    Ok(ShaderStencilOp {
        pass: shader_value(op.get("pass"), "stencil pass")?,
        fail: shader_value(op.get("fail"), "stencil fail")?,
        z_fail: shader_value(op.get("zFail"), "stencil zFail")?,
        comp: shader_value(op.get("comp"), "stencil comp")?,
    })
}

/// A pass's render state beyond the first target's blend, depth write and culling (`m_State`).
fn pass_state(st: &Value, name: &str) -> Result<PassState> {
    let mut targets = Vec::with_capacity(8);
    for n in 0..8 {
        let rt = st
            .get(&format!("rtBlend{n}"))
            .ok_or_else(|| invalid(format!("shader {name}: a pass without rtBlend{n}")))?;
        targets.push(target_blend(rt)?);
    }
    Ok(PassState {
        targets,
        separate_blend: st
            .get("rtSeparateBlend")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid(format!("shader {name}: a pass without rtSeparateBlend")))?,
        z_test: shader_value(st.get("zTest"), "zTest")?,
        offset_factor: shader_value(st.get("offsetFactor"), "offsetFactor")?,
        offset_units: shader_value(st.get("offsetUnits"), "offsetUnits")?,
        stencil_ref: shader_value(st.get("stencilRef"), "stencilRef")?,
        stencil_read_mask: shader_value(st.get("stencilReadMask"), "stencilReadMask")?,
        stencil_write_mask: shader_value(st.get("stencilWriteMask"), "stencilWriteMask")?,
        stencil_op: stencil_op(st, "stencilOp")?,
        stencil_op_front: stencil_op(st, "stencilOpFront")?,
        stencil_op_back: stencil_op(st, "stencilOpBack")?,
    })
}

impl ShaderData {
    /// Read the Shader `r` of `player`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let s = player.decode(r)?;
        let name =
            shader_name_of(&s).ok_or_else(|| invalid(format!("shader {}/{}: no name", r.file, r.path_id)))?;
        let mut subshaders = Vec::new();
        if let Some(form) = s.get("m_ParsedForm") {
            for sub in list(form, "m_SubShaders")? {
                let mut passes = Vec::new();
                for p in list(sub, "m_Passes")? {
                    let st = p
                        .get("m_State")
                        .ok_or_else(|| invalid(format!("shader {name}: a pass without m_State")))?;
                    let rt = st
                        .get("rtBlend0")
                        .ok_or_else(|| invalid(format!("shader {name}: a pass without rtBlend0")))?;
                    let kind = match p.get("m_Type").and_then(Value::as_i64) {
                        Some(0) => PassKind::Normal,
                        Some(1) => PassKind::Use(
                            p.get("m_UseName")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    invalid(format!("shader {name}: a UsePass without m_UseName"))
                                })?
                                .to_owned(),
                        ),
                        Some(2) => PassKind::Grab,
                        t => return Err(invalid(format!("shader {name}: pass type {t:?}"))),
                    };
                    passes.push(ShaderPass {
                        kind,
                        light_mode: tag_list(st.get("m_Tags"))?
                            .into_iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("LightMode"))
                            .map(|(_, v)| v),
                        src_blend: shader_value(rt.get("srcBlend"), "srcBlend")?,
                        dst_blend: shader_value(rt.get("destBlend"), "destBlend")?,
                        zwrite: shader_value(st.get("zWrite"), "zWrite")?,
                        cull: shader_value(st.get("culling"), "culling")?,
                        state: pass_state(st, &name)?,
                    });
                }
                subshaders.push(SubShader {
                    tags: tag_list(sub.get("m_Tags"))?,
                    passes,
                });
            }
        }
        Ok(ShaderData { name, subshaders })
    }
}

fn shader_name_of(s: &Value) -> Option<String> {
    s.at(&["m_ParsedForm", "m_Name"])
        .or_else(|| s.get("m_Name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The name of the Shader `r` (`m_ParsedForm.m_Name`).
pub fn shader_name(player: &Player, r: &ObjRef) -> Result<String> {
    shader_name_of(&player.decode(r)?)
        .ok_or_else(|| invalid(format!("shader {}/{}: no name", r.file, r.path_id)))
}
