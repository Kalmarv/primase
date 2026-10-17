//! Unity materials as stored: the shader's name and render state, and the saved properties (textures with their tiling
//! and offset, floats, integers, colours), keywords, tags and render queue. What they mean depends on the shader;
//! lowering to `pbr.material` is [`crate::lower`].

use crate::player::{ObjRef, Player};
use crate::value::{Array, Value};
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
            keywords: strings(&m, "m_ValidKeywords")?,
            tags: pairs(&m, "stringTagMap", |s| s.as_str().map(str::to_owned))?,
            render_queue: m.get("m_CustomRenderQueue").and_then(Value::as_i64).unwrap_or(-1),
        })
    }
}

/// A render-state value of a shader pass: the value the shader declares, and the material property it is bound to
/// (`Blend [_SrcBlend] ...`), which then gives the value.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderValue {
    pub value: f32,
    pub property: Option<String>,
}

/// A shader pass's render state: its `LightMode` tag, the first render target's blend factors (Unity's `BlendMode`:
/// 0 Zero, 1 One, 5 SrcAlpha, 10 OneMinusSrcAlpha, ...), depth writes and culling (0 off, 1 front, 2 back).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShaderPass {
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
                    passes.push(ShaderPass {
                        light_mode: tag_list(st.get("m_Tags"))?
                            .into_iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("LightMode"))
                            .map(|(_, v)| v),
                        src_blend: shader_value(rt.get("srcBlend"), "srcBlend")?,
                        dst_blend: shader_value(rt.get("destBlend"), "destBlend")?,
                        zwrite: shader_value(st.get("zWrite"), "zWrite")?,
                        cull: shader_value(st.get("culling"), "culling")?,
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
