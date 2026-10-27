//! What one bake evaluates, as data ([`PipelineState`]): the programs as stored, their constant buffers as words, the
//! textures as the texels they sample, what the vertex program is run on and what the pixel program is given. A bake
//! builds its state first and then evaluates the state and nothing else ([`super::Prepared::pipeline_state`]), so the
//! state a caller takes is the one the bake ran: another executor (a rasterizer running the same programs) can be
//! given exactly it.
//!
//! The values in it are the bake's policy (the frozen frame, the vertex feeds, the layer weights; counted in
//! [`super::Prepared::policies`]); what another executor can check against it is the execution: the vertex program's
//! outputs at the corners, the pixel program over the grid, the interpolation and the sampling.
use std::collections::BTreeMap;
use std::sync::Arc;

use iridia_exec::{Input, Lane, View, Views, evaluate::Constants};
use primase_foundations::sampling::Address;
use serde_json::json;

use crate::invalid;
use fromsoft_formats::Result;
use iridia_exec::SampledTexture;

/// The pixel program's outputs a bake reads, as (output register, component mask): the G-buffer normal, the second
/// output's x, the albedo, the reflectance and the emission.
pub const TARGETS: [(u32, u8); 5] = [(0, 7), (1, 1), (2, 7), (3, 7), (4, 7)];

/// A texture slot's content.
#[derive(Clone, Debug)]
pub enum SlotTexture {
    /// A texture's base mip, sampled bilinearly with its addressing ([`SampledTexture`]).
    Image(Arc<SampledTexture>),
    /// One value everywhere: an unbound slot's preview colour, or the engine's dither texture.
    Constant([f32; 4]),
}

/// The pixel stage of a bake.
#[derive(Clone, Debug)]
pub struct PixelState {
    /// The program's container as stored.
    pub bytecode: Arc<[u8]>,
    /// Every constant buffer the program reads, by slot ([`Constants::buffers`]).
    pub constants: Constants,
    /// Every texture slot the program samples, by `t` register.
    pub textures: BTreeMap<u32, SlotTexture>,
    /// What each used input is given, by (semantic, index): each lane its bits, or the grid's U or V
    /// ([`iridia_exec::Grid`]: `(x + 0.5) / size * period[0]` along x, likewise V along y).
    pub inputs: BTreeMap<(String, u32), [Lane; 4]>,
}

/// The vertex stage of a bake whose material has a vertex program: the program run once at each corner of a texture
/// cell ([`super::vertex`]), where the pixel program's inputs are read from what it writes there.
#[derive(Clone, Debug)]
pub struct VertexState {
    /// The program's container as stored.
    pub bytecode: Arc<[u8]>,
    /// Every constant buffer the program reads, by slot, as rows of four words.
    pub constants: Vec<(u32, Vec<[u32; 4]>)>,
    /// The input registers of each run, at the cell's corners `(u, v)` = (0, 0), (1, 0), (0, 1) and (1, 1) in this
    /// order. Every structured buffer the program reads gives zero (one instance, index zero).
    pub corners: [Vec<[u32; 4]>; 4],
    /// The output registers each run wrote, in the same order: the varyings by register. A system value's register (the
    /// position, the clip distances) is the pipeline's and is not computed here: it reads zero.
    pub outputs: [Vec<[u32; 4]>; 4],
}

/// The albedo's alpha, which the bake samples beside the program (the program's own alpha is not the cutout): the main
/// albedo at `scale` times the grid's coordinate.
#[derive(Clone, Debug)]
pub struct AlphaState {
    pub image: Arc<SampledTexture>,
    pub scale: [f32; 2],
}

/// Everything one bake evaluates.
#[derive(Clone, Debug)]
pub struct PipelineState {
    /// The grid: `size` x `size` pixels over `period` texture units along u and v.
    pub size: u32,
    pub period: [f32; 2],
    pub pixel: PixelState,
    /// `None` where the material has no vertex program: the pixel inputs are then the fixed table's.
    pub vertex: Option<VertexState>,
    /// `None` where the material has no main albedo: the alpha is opaque.
    pub alpha: Option<AlphaState>,
    /// The pixel inputs where the vertex program's differ from the fixed table's, by input and the two values: counted
    /// beside the bake's policies.
    pub differences: BTreeMap<String, u64>,
}

impl PixelState {
    /// The textures as the executor samples them.
    pub(super) fn evaluated_textures(&self) -> Views {
        Views::refusing(
            self.textures
                .iter()
                .map(|(&slot, t)| {
                    (
                        slot,
                        match t {
                            SlotTexture::Image(image) => {
                                View::Image(image.clone() as Arc<dyn iridia_exec::Image>)
                            }
                            SlotTexture::Constant(value) => View::Constant(*value),
                        },
                    )
                })
                .collect(),
        )
    }

    /// The inputs as the executor takes them.
    pub(super) fn evaluated_inputs(&self) -> BTreeMap<(String, u32), Input> {
        self.inputs
            .iter()
            .map(|(key, lanes)| (key.clone(), Input::Lanes(*lanes)))
            .collect()
    }
}

/// What [`PipelineState::encode`] writes: a description (JSON) and the files it names, by name.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub description: serde_json::Value,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// The description's `schema` value.
pub const SCHEMA: &str = "primase layer-bake pipeline state 1";

fn hex(bits: u32) -> serde_json::Value {
    format!("{bits:08x}").into()
}

fn bits(value: &serde_json::Value) -> Result<u32> {
    value
        .as_str()
        .and_then(|s| u32::from_str_radix(s, 16).ok())
        .ok_or_else(|| invalid("layer bake state", 0, format!("{value} is not a word in hex")))
}

fn address_name(address: Address) -> &'static str {
    match address {
        Address::Wrap => "wrap",
        Address::Clamp => "clamp",
        Address::Mirror => "mirror",
        Address::MirrorOnce => "mirror-once",
    }
}

fn address_of(value: &serde_json::Value) -> Result<Address> {
    Ok(match value.as_str() {
        Some("wrap") => Address::Wrap,
        Some("clamp") => Address::Clamp,
        Some("mirror") => Address::Mirror,
        Some("mirror-once") => Address::MirrorOnce,
        _ => return Err(invalid("layer bake state", 0, format!("addressing {value}"))),
    })
}

fn rows_json(rows: &[(u32, Vec<[u32; 4]>)]) -> serde_json::Value {
    rows.iter()
        .map(|(slot, rows)| json!({"slot": slot, "words": rows.iter().flatten().map(|&w| hex(w)).collect::<Vec<_>>()}))
        .collect()
}

fn rows_of(value: &serde_json::Value) -> Result<Vec<(u32, Vec<[u32; 4]>)>> {
    array(value)?
        .iter()
        .map(|b| {
            let words = array(&b["words"])?.iter().map(bits).collect::<Result<Vec<_>>>()?;
            if !words.len().is_multiple_of(4) {
                return Err(invalid(
                    "layer bake state",
                    0,
                    "a buffer's words are not whole rows",
                ));
            }
            Ok((
                number(&b["slot"])?,
                words.chunks(4).map(|r| [r[0], r[1], r[2], r[3]]).collect(),
            ))
        })
        .collect()
}

fn registers_json(runs: &[Vec<[u32; 4]>; 4]) -> serde_json::Value {
    runs.iter()
        .map(|registers| {
            registers
                .iter()
                .map(|r| r.iter().map(|&w| hex(w)).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn registers_of(value: &serde_json::Value) -> Result<[Vec<[u32; 4]>; 4]> {
    let runs = array(value)?
        .iter()
        .map(|run| {
            array(run)?
                .iter()
                .map(|r| {
                    let lanes = array(r)?.iter().map(bits).collect::<Result<Vec<_>>>()?;
                    <[u32; 4]>::try_from(lanes)
                        .map_err(|_| invalid("layer bake state", 0, "a register of other than four lanes"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    <[Vec<[u32; 4]>; 4]>::try_from(runs).map_err(|_| invalid("layer bake state", 0, "not four corner runs"))
}

fn array(value: &serde_json::Value) -> Result<&Vec<serde_json::Value>> {
    value
        .as_array()
        .ok_or_else(|| invalid("layer bake state", 0, format!("{value} is not an array")))
}

fn number(value: &serde_json::Value) -> Result<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid("layer bake state", 0, format!("{value} is not a count")))
}

impl PipelineState {
    /// The state as a description and the files it names: the programs' containers (`pixel.dxbc`, `vertex.dxbc`) and
    /// each image's stored texels (`t<slot>.rgba`, `alpha.rgba`, RGBA8 in rows). Words and floats are written as their
    /// bits in hex, so nothing is rounded; a lane is `"U"`, `"V"` or its bits.
    pub fn encode(&self) -> Encoded {
        let mut files = BTreeMap::new();
        let mut image = |name: String, image: &SampledTexture| {
            let [width, height] = image.dimensions();
            files.insert(name.clone(), super::stored(image).to_vec());
            json!({
                "file": name,
                "width": width,
                "height": height,
                "srgb": super::srgb(image),
                "address": image.address().map(address_name),
            })
        };
        let textures: Vec<_> = self
            .pixel
            .textures
            .iter()
            .map(|(&slot, t)| match t {
                SlotTexture::Image(i) => json!({"slot": slot, "image": image(format!("t{slot}.rgba"), i)}),
                SlotTexture::Constant(value) => {
                    json!({"slot": slot, "constant": value.iter().map(|v| hex(v.to_bits())).collect::<Vec<_>>()})
                }
            })
            .collect();
        let alpha = self.alpha.as_ref().map(|a| {
            json!({"image": image("alpha.rgba".into(), &a.image), "scale": a.scale.map(|v| hex(v.to_bits()))})
        });
        let inputs: Vec<_> = self
            .pixel
            .inputs
            .iter()
            .map(|((semantic, index), lanes)| {
                json!({
                    "semantic": semantic,
                    "index": index,
                    "lanes": lanes.iter().map(|l| match l {
                        Lane::U => "U".into(),
                        Lane::V => "V".into(),
                        Lane::Bits(b) => hex(*b),
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        files.insert("pixel.dxbc".into(), self.pixel.bytecode.to_vec());
        let vertex = self.vertex.as_ref().map(|v| {
            files.insert("vertex.dxbc".into(), v.bytecode.to_vec());
            json!({
                "bytecode": "vertex.dxbc",
                "constants": rows_json(&v.constants),
                "corners": registers_json(&v.corners),
                "outputs": registers_json(&v.outputs),
            })
        });
        let description = json!({
            "schema": SCHEMA,
            "size": self.size,
            "period": self.period.map(|v| hex(v.to_bits())),
            "pixel": {
                "bytecode": "pixel.dxbc",
                "constants": rows_json(self.pixel.constants.buffers()),
                "textures": textures,
                "inputs": inputs,
                "targets": TARGETS,
            },
            "vertex": vertex,
            "alpha": alpha,
            "differences": self.differences,
        });
        Encoded { description, files }
    }

    /// The state an [`Encoded`] describes ([`PipelineState::encode`]). The pixel program is lifted to lay out its
    /// constant buffers; a buffer the program does not read, or words past one it reads, are an error.
    pub fn decode(encoded: &Encoded) -> Result<PipelineState> {
        let d = &encoded.description;
        if d["schema"] != SCHEMA {
            return Err(invalid("layer bake state", 0, format!("schema {}", d["schema"])));
        }
        let file = |name: &serde_json::Value| -> Result<&Vec<u8>> {
            name.as_str()
                .and_then(|n| encoded.files.get(n))
                .ok_or_else(|| invalid("layer bake state", 0, format!("no file {name}")))
        };
        let image = |i: &serde_json::Value| -> Result<Arc<SampledTexture>> {
            let address = array(&i["address"])?;
            let [u, v] = address.as_slice() else {
                return Err(invalid(
                    "layer bake state",
                    0,
                    "addressing of other than two axes",
                ));
            };
            Ok(Arc::new(super::base_texture(
                number(&i["width"])?,
                number(&i["height"])?,
                file(&i["file"])?,
                i["srgb"]
                    .as_bool()
                    .ok_or_else(|| invalid("layer bake state", 0, "srgb"))?,
                [address_of(u)?, address_of(v)?],
            )?))
        };
        let pair = |v: &serde_json::Value| -> Result<[f32; 2]> {
            let a = array(v)?;
            let [x, y] = a.as_slice() else {
                return Err(invalid("layer bake state", 0, "not a pair"));
            };
            Ok([f32::from_bits(bits(x)?), f32::from_bits(bits(y)?)])
        };
        let pixel = &d["pixel"];
        let bytecode: Arc<[u8]> = file(&pixel["bytecode"])?.as_slice().into();
        let lifted = iridia_exec::ProgramShader::new(
            iridia_dxbc::lift(&bytecode).map_err(|e| invalid("layer bake state", e.at * 4, e.detail))?,
        )
        .map_err(|e| invalid("layer bake state", e.at, e.detail))?;
        let layout =
            iridia_exec::Layout::of(&lifted).map_err(|e| invalid("layer bake state", e.at, e.detail))?;
        let mut constants = Constants::new(&layout);
        for (slot, rows) in rows_of(&pixel["constants"])? {
            let words: Vec<u32> = rows.into_iter().flatten().collect();
            constants
                .set_words(slot, 0, &words)
                .map_err(|e| invalid("layer bake state", e.at, e.detail))?;
        }
        let mut textures = BTreeMap::new();
        for t in array(&pixel["textures"])? {
            let slot = number(&t["slot"])?;
            let texture = if t["image"].is_object() {
                SlotTexture::Image(image(&t["image"])?)
            } else {
                let lanes = array(&t["constant"])?
                    .iter()
                    .map(|v| bits(v).map(f32::from_bits))
                    .collect::<Result<Vec<_>>>()?;
                SlotTexture::Constant(
                    <[f32; 4]>::try_from(lanes)
                        .map_err(|_| invalid("layer bake state", 0, "a constant of other than four lanes"))?,
                )
            };
            textures.insert(slot, texture);
        }
        let mut inputs = BTreeMap::new();
        for i in array(&pixel["inputs"])? {
            let lanes = array(&i["lanes"])?
                .iter()
                .map(|l| match l.as_str() {
                    Some("U") => Ok(Lane::U),
                    Some("V") => Ok(Lane::V),
                    _ => bits(l).map(Lane::Bits),
                })
                .collect::<Result<Vec<_>>>()?;
            let semantic = i["semantic"]
                .as_str()
                .ok_or_else(|| invalid("layer bake state", 0, "an input without a semantic"))?;
            inputs.insert(
                (semantic.to_owned(), number(&i["index"])?),
                <[Lane; 4]>::try_from(lanes)
                    .map_err(|_| invalid("layer bake state", 0, "an input of other than four lanes"))?,
            );
        }
        let vertex = if d["vertex"].is_null() {
            None
        } else {
            let v = &d["vertex"];
            Some(VertexState {
                bytecode: file(&v["bytecode"])?.as_slice().into(),
                constants: rows_of(&v["constants"])?,
                corners: registers_of(&v["corners"])?,
                outputs: registers_of(&v["outputs"])?,
            })
        };
        let alpha = if d["alpha"].is_null() {
            None
        } else {
            Some(AlphaState {
                image: image(&d["alpha"]["image"])?,
                scale: pair(&d["alpha"]["scale"])?,
            })
        };
        let differences = d["differences"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(PipelineState {
            size: number(&d["size"])?,
            period: pair(&d["period"])?,
            pixel: PixelState {
                bytecode,
                constants,
                textures,
                inputs,
            },
            vertex,
            alpha,
            differences,
        })
    }
}
