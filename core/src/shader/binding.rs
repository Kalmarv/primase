//! `core.shader_binding`: on a material entity, which shader programs draw it and what each thing they read stands
//! for. It states roles, never frozen values: a bake or a renderer decides what a role is worth (a camera position,
//! the time, the frame's exposure) and says so.

use serde::Serialize;
use serde_json::Value;

use crate::shader::{
    BufferBinding, BufferSource, BufferTable, ConstantBinding, ConstantRole, FrameTexture, InputBinding,
    ObjectParameter, OutputBinding, OutputRole, VariantKey,
};
use iridia_interface::{
    BufferKind, ConstantBufferBinding, Interface, Label, MatrixLines, RenderState, ResourceId, ResourceKind,
    SamplerState, Stage, SystemValue, Unshown,
};

use super::Shader;
use crate::TextureLayers;
use crate::components::{Texture, TextureFile};
use crate::json::JsonError;
use crate::world::{Component, Entity};

/// What a texture slot samples.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureSource {
    /// A texture entity (`core.texture`, `core.texture_layers` or `core.texture_file`), with the sampler state stored
    /// with it.
    Texture { texture: Entity, sampler: SamplerState },
    /// No texture; the slot reads this value everywhere, and why.
    Default { value: [f32; 4], why: String },
    /// An image of the frame being drawn, which the consumer supplies.
    Frame(FrameTexture),
    /// Nothing is bound and the source says no default: a consumer must not invent one.
    Missing,
    /// A view of one of the frame's own targets, read through the sampler state the source binds with it: only in the
    /// binding of a step of the frame's `core.render_graph` ([`crate::render_graph`]), whose targets these are. A
    /// material's binding never states one.
    Target {
        view: crate::render_graph::TargetView,
        sampler: SamplerState,
    },
}
/// Where a stage binding stands, which decides what its texture slots may bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingPlace {
    /// A material's `core.shader_binding`.
    Material,
    /// A step of the frame's `core.render_graph`, which may bind the graph's targets.
    GraphStep,
}
/// One texture slot: the texture resource of the shader, the sampler resource it is read through when the program
/// has one, and what is bound.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextureBinding {
    pub resource: ResourceId,
    pub sampler: Option<ResourceId>,
    pub source: TextureSource,
}
/// One shader program in a material, with the role of everything it reads and writes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StageBinding {
    pub stage: Stage,
    /// The variant of the draw this stage is for, where the source draws one material with different programs of a
    /// stage depending on the mesh (its vertex layout, whether it is skinned): the key the mesh's own data must equal
    /// ([`VariantKey`]). A binding holds one stage of each kind, or several of one kind each for a distinct value of one
    /// key.
    pub variant: Option<VariantKey>,
    /// The `core.shader` entity.
    pub shader: Entity,
    pub inputs: Vec<InputBinding>,
    pub constants: Vec<ConstantBinding>,
    pub textures: Vec<TextureBinding>,
    /// The buffers the program indexes at run time (tables of the draw's matrices and instances). A stage that states
    /// any states every buffer resource of its program (a table, or [`BufferSource::Unknown`] with why); empty for a
    /// source that states none, whose buffers a consumer takes as [`StageBinding::buffer`] says: unknown, with the
    /// reason it prints.
    pub buffers: Vec<BufferBinding>,
    /// The constant buffers the source binds at other than their declared size ([`ConstantBufferBinding`]); a buffer
    /// with no entry is bound at its declared size.
    pub constant_buffers: Vec<ConstantBufferBinding>,
    pub outputs: Vec<OutputBinding>,
}
/// `core.shader_binding`, on a material entity: the shader programs it is drawn with, one per stage, what each reads
/// and writes, and its render state. Everything a program reads has a role, never a frozen value: textures are the
/// entities bound, constants are roles with the material's own words as stored, views and frames are named, not
/// valued. What can't be resolved is `Unknown` with the reason, or `Missing`, so nothing is silently made up.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ShaderBinding {
    /// At most one binding per stage.
    pub stages: Vec<StageBinding>,
    pub state: RenderState,
}
impl ShaderBinding {
    /// The binding of this stage.
    pub fn stage(&self, stage: Stage) -> Option<&StageBinding> {
        self.stages.iter().find(|s| s.stage == stage)
    }
}
impl StageBinding {
    /// The constants whose role is [`ConstantRole::Unknown`], by name (or offset when the source names none) and why.
    /// A consumer that evaluates the stage uses it to refuse, or to say which values it left out, where zero is not a
    /// neutral value (a scale, say): it never reads an unknown constant as zero without naming it.
    pub fn unknown_constants(&self) -> impl Iterator<Item = (String, &str)> {
        self.constants.iter().filter_map(|c| match &c.role {
            ConstantRole::Unknown { why } => Some((
                c.name.clone().unwrap_or_else(|| format!("+{}", c.offset)),
                why.as_str(),
            )),
            _ => None,
        })
    }

    /// The lanes the program writes that no output binding states, per output of `interface` (the stage's program's):
    /// written as its source states it, the D3D signature's lanes less those it never writes (DXBC's `OSGN`, DXIL's
    /// `OSG1`), in the output's own lanes (bit k its lane k). An output whose source states no written lanes (no D3D
    /// signature: MojoShader's, SPIR-V's and GLSL's programs) has none here, nor a system value other than a render
    /// target (a depth, a coverage mask), which states what it is itself.
    pub fn unbound_written_lanes<'i>(&self, interface: &'i Interface) -> Vec<(&'i Label, u8)> {
        let mut out = Vec::new();
        for v in &interface.outputs {
            if v.system.is_some_and(|s| !matches!(s, SystemValue::Target(_))) {
                continue;
            }
            let Some(d3d) = v.provenance.d3d else {
                continue;
            };
            // the element's own lanes: the register's from its first, as many as it is wide
            let first = d3d.mask.trailing_zeros();
            let width = u32::from(v.value.width).min(4);
            let written = ((d3d.mask & !d3d.read_write_mask) >> first) & ((1u8 << width) - 1);
            let bound = self
                .outputs
                .iter()
                .filter(|o| o.label == v.label)
                .fold(0u8, |acc, o| acc | o.lanes);
            let unbound = written & !bound;
            if unbound != 0 {
                out.push((&v.label, unbound));
            }
        }
        out
    }

    /// The binding agrees with its program: a material's ([`BindingPlace::Material`]) binds no target of a render graph.
    pub(crate) fn verify_in(&self, w: &crate::World, place: BindingPlace) -> Result<(), String> {
        let shader = w
            .get::<Shader>(self.shader)
            .ok_or_else(|| format!("the {:?} stage's shader entity has no core.shader", self.stage))?;
        let interface: &Interface = &shader.interface;
        let at = |detail: String| format!("{:?} stage: {detail}", self.stage);
        if interface.stage != self.stage {
            return Err(at(format!("its shader is a {:?} program", interface.stage)));
        }
        for (n, i) in self.inputs.iter().enumerate() {
            if !interface.inputs.iter().any(|v| v.label == i.label) {
                return Err(at(format!(
                    "input {}{} is not in the shader",
                    i.label.name, i.label.index
                )));
            }
            if self.inputs[..n].iter().any(|before| before.label == i.label) {
                return Err(at(format!(
                    "input {}{} is bound twice",
                    i.label.name, i.label.index
                )));
            }
        }
        for (n, o) in self.outputs.iter().enumerate() {
            let Some(v) = interface.outputs.iter().find(|v| v.label == o.label) else {
                return Err(at(format!(
                    "output {}{} is not in the shader",
                    o.label.name, o.label.index
                )));
            };
            if o.lanes == 0 || u32::from(o.lanes) >> v.value.width != 0 {
                return Err(at(format!(
                    "output {}{} has {} lanes, not the mask {:#x}",
                    o.label.name, o.label.index, v.value.width, o.lanes
                )));
            }
            if let Some(Err(why)) = o.role.scalar_encoding().map(|e| e.check()) {
                return Err(at(format!("output {}{}: {why}", o.label.name, o.label.index)));
            }
            if let OutputRole::Unknown { why } = &o.role
                && why.trim().is_empty()
            {
                return Err(at(format!(
                    "output {}{} is Unknown with no why",
                    o.label.name, o.label.index
                )));
            }
            // one label may carry several roles, each on its own lanes
            if let Some(before) = self.outputs[..n]
                .iter()
                .find(|before| before.label == o.label && before.lanes & o.lanes != 0)
            {
                return Err(at(format!(
                    "output {}{} has two roles on lanes {:#x}",
                    o.label.name,
                    o.label.index,
                    before.lanes & o.lanes
                )));
            }
        }
        // a pixel program's outputs are the draw's targets: each lane it writes has a role, or is Unknown with why. Any
        // other stage's outputs are the next stage's inputs, bound there.
        if self.stage == Stage::Pixel
            && let Some((label, lanes)) = self.unbound_written_lanes(interface).first()
        {
            return Err(at(format!(
                "output {}{} lanes {lanes:#06b} are written and have no binding",
                label.name, label.index
            )));
        }
        for c in &self.constants {
            let Some(ResourceKind::ConstantBuffer { layout }) =
                interface.resource(c.resource).map(|r| &r.kind)
            else {
                return Err(at(format!(
                    "constant resource {} is not a constant buffer",
                    c.resource.0
                )));
            };
            if c.size == 0 || c.offset.checked_add(c.size).is_none_or(|end| end > layout.size) {
                return Err(at(format!(
                    "constant of {} bytes at {} is outside buffer {} of {} bytes",
                    c.size, c.offset, c.resource.0, layout.size
                )));
            }
            let named = c.name.as_deref().map_or(String::new(), |n| format!("{n} "));
            match (c.role.is_matrix(), &c.lines) {
                (true, None) => {
                    return Err(at(format!(
                        "constant {named}at {} is a matrix ({:?}) whose lines are not stated",
                        c.offset, c.role
                    )));
                }
                (true, Some(MatrixLines::Unshown(Unshown::Unexamined { why }))) if why.trim().is_empty() => {
                    return Err(at(format!(
                        "constant {named}at {} is a matrix whose lines are unshown with no reason",
                        c.offset
                    )));
                }
                (false, Some(lines)) => {
                    return Err(at(format!(
                        "constant {named}at {} states lines ({lines:?}) for a role that is no matrix ({:?})",
                        c.offset, c.role
                    )));
                }
                _ => {}
            }
            if let ConstantRole::AlphaReference { value: r, .. } = c.role
                && (!r.is_finite() || c.size != 4)
            {
                return Err(at(format!(
                    "the alpha reference {r} at {} is not one finite float ({} bytes)",
                    c.offset, c.size
                )));
            }
            if let ConstantRole::Material(words) = &c.role
                && words.len() as u64 * 4 > u64::from(c.size)
            {
                return Err(at(format!(
                    "{} material words for a constant of {} bytes at {}",
                    words.len(),
                    c.size,
                    c.offset
                )));
            }
        }
        // the bytes of a constant buffer belong to one constant
        let mut spans: Vec<(u32, u32, u32)> = self
            .constants
            .iter()
            .map(|c| (c.resource.0, c.offset, c.offset + c.size))
            .collect();
        spans.sort_unstable();
        if let Some(pair) = spans.windows(2).find(|p| p[0].0 == p[1].0 && p[1].1 < p[0].2) {
            return Err(at(format!(
                "constants overlap in buffer {}: bytes {}..{} and {}..{}",
                pair[0].0, pair[0].1, pair[0].2, pair[1].1, pair[1].2
            )));
        }
        for (n, t) in self.textures.iter().enumerate() {
            let Some(ResourceKind::Texture { dim, arrayed, .. }) =
                interface.resource(t.resource).map(|r| &r.kind)
            else {
                return Err(at(format!("resource {} is not a texture", t.resource.0)));
            };
            if self.textures[..n]
                .iter()
                .any(|before| before.resource == t.resource)
            {
                return Err(at(format!("texture slot {} is bound twice", t.resource.0)));
            }
            if place == BindingPlace::Material && matches!(t.source, TextureSource::Target { .. }) {
                return Err(at(format!(
                    "texture slot {} binds a render graph target, which only a step of a core.render_graph may",
                    t.resource.0
                )));
            }
            if let TextureSource::Frame(frame) = &t.source
                && *dim != frame.dim()
            {
                return Err(at(format!(
                    "texture slot {} is a {dim:?} texture and binds the frame's {frame:?}, a {:?} image",
                    t.resource.0,
                    frame.dim()
                )));
            }
            if let Some(s) = t.sampler
                && !matches!(
                    interface.resource(s).map(|r| &r.kind),
                    Some(ResourceKind::Sampler { .. })
                )
            {
                return Err(at(format!("resource {} is not a sampler", s.0)));
            }
            if let TextureSource::Texture { texture, .. } = &t.source {
                if !(w.has::<Texture>(*texture)
                    || w.has::<TextureLayers>(*texture)
                    || w.has::<TextureFile>(*texture))
                {
                    return Err(at(format!(
                        "texture slot {} binds an entity that is neither core.texture, core.texture_layers nor core.texture_file",
                        t.resource.0
                    )));
                }
                // a texture of several images is read as the dimension its shape is
                if let Some(layers) = w.get::<TextureLayers>(*texture)
                    && layers.shape.dimension() != (*dim, *arrayed)
                {
                    return Err(at(format!(
                        "texture slot {} is a {dim:?} resource (arrayed {arrayed}) and binds a {:?} texture",
                        t.resource.0, layers.shape
                    )));
                }
            }
        }
        self.verify_buffers(interface, place).map_err(at)?;
        self.verify_constant_buffers(interface).map_err(at)
    }

    /// What buffer `resource` of the program holds: its entry, or, where the source states no buffers for the stage,
    /// unknown with that reason (never zeros without saying so).
    pub fn buffer(&self, resource: ResourceId) -> BufferSource {
        self.buffers
            .iter()
            .find(|b| b.resource == resource)
            .map(|b| b.source.clone())
            .unwrap_or_else(|| BufferSource::Unknown {
                why: "the source states no buffers for this stage".into(),
            })
    }

    /// The constant buffers the source states a binding for: each a constant buffer of the shader, once, bound at a whole
    /// number of rows no smaller than the program declares.
    fn verify_constant_buffers(&self, interface: &Interface) -> Result<(), String> {
        for (n, c) in self.constant_buffers.iter().enumerate() {
            let Some(ResourceKind::ConstantBuffer { layout }) =
                interface.resource(c.resource).map(|r| &r.kind)
            else {
                return Err(format!("resource {} is not a constant buffer", c.resource.0));
            };
            if self.constant_buffers[..n]
                .iter()
                .any(|before| before.resource == c.resource)
            {
                return Err(format!("constant buffer {} is bound twice", c.resource.0));
            }
            if let Some(bytes) = c.bound_size
                && (bytes % 16 != 0 || bytes < layout.size)
            {
                return Err(format!(
                    "constant buffer {} is bound at {bytes} bytes: not a whole number of 16-byte rows, or smaller than the {} the program declares",
                    c.resource.0, layout.size
                ));
            }
        }
        Ok(())
    }

    /// The bytes the constant buffer `resource` is bound with where the source states a size, else `None` (its
    /// declared size).
    pub fn bound_size(&self, resource: ResourceId) -> Option<u32> {
        self.constant_buffers
            .iter()
            .find(|c| c.resource == resource)
            .and_then(|c| c.bound_size)
    }

    /// How many matrices the joint palette constant holds as the program reads it: the stage's
    /// [`ObjectParameter::BonePalette`] constant held as rows (three 16-byte lines a matrix, 48 bytes), at its size or, where
    /// the source binds its buffer larger than the program declares ([`Self::bound_size`]), the bytes from its offset to the
    /// bound size, whichever is more. `None` where the stage has no such constant, or holds it as columns or does not show
    /// which (the lab and the census fill rows only).
    pub fn bone_palette_matrices(&self) -> Option<u32> {
        let c = self.constants.iter().find(|c| {
            matches!(c.role, ConstantRole::Object(ObjectParameter::BonePalette))
                && matches!(c.lines, Some(MatrixLines::Rows))
        })?;
        let bound = self
            .bound_size(c.resource)
            .map_or(0, |b| b.saturating_sub(c.offset));
        Some(c.size.max(bound) / 48)
    }

    /// The buffer slots: each a buffer of the shader, bound once, a matrix table of 16-byte elements, and an instance
    /// table whose records fit its stride and name a matrix table of this stage; a stage that states any buffer states
    /// every buffer of its program.
    fn verify_buffers(&self, interface: &Interface, place: BindingPlace) -> Result<(), String> {
        let kind = |id: ResourceId| match interface.resource(id).map(|r| &r.kind) {
            Some(ResourceKind::Buffer(kind)) => Some(*kind),
            _ => None,
        };
        for (n, b) in self.buffers.iter().enumerate() {
            let Some(buffer) = kind(b.resource) else {
                return Err(format!("resource {} is not a buffer", b.resource.0));
            };
            if self.buffers[..n]
                .iter()
                .any(|before| before.resource == b.resource)
            {
                return Err(format!("buffer slot {} is bound twice", b.resource.0));
            }
            match &b.source {
                BufferSource::Unknown { why } if why.is_empty() => {
                    return Err(format!(
                        "buffer slot {} is unknown and says not why",
                        b.resource.0
                    ));
                }
                BufferSource::Unknown { .. } => {}
                BufferSource::Graph(_) if place == BindingPlace::Material => {
                    return Err(format!(
                        "buffer slot {} binds a render graph buffer, which only a step of a core.render_graph may",
                        b.resource.0
                    ));
                }
                BufferSource::Graph(_) => {}
                BufferSource::Table(
                    BufferTable::ObjectMatrices { rows }
                    | BufferTable::HeaderedMatrices { rows }
                    | BufferTable::SkinnedMatrices { rows, .. },
                ) => {
                    if !(3..=4).contains(rows) {
                        return Err(format!(
                            "buffer slot {} holds matrices of {rows} rows, not 3 or 4",
                            b.resource.0
                        ));
                    }
                    if let BufferKind::Structured { stride } = buffer
                        && stride != 16
                    {
                        return Err(format!(
                            "buffer slot {} holds matrix rows of 16 bytes and has elements of {stride}",
                            b.resource.0
                        ));
                    }
                    if let BufferSource::Table(BufferTable::SkinnedMatrices {
                        joint_stride, prefix, ..
                    }) = &b.source
                    {
                        if joint_stride < rows {
                            return Err(format!(
                                "buffer slot {} holds joints {joint_stride} elements apart with {rows} rows each",
                                b.resource.0
                            ));
                        }
                        if *prefix == 0 {
                            return Err(format!(
                                "buffer slot {} holds skinned matrices with no prefix element to say what the first one holds",
                                b.resource.0
                            ));
                        }
                    }
                }
                BufferSource::Table(BufferTable::Instances {
                    stride,
                    matrix_start,
                    matrices,
                }) => {
                    if *stride == 0 || matrix_start >= stride {
                        return Err(format!(
                            "buffer slot {} holds instance records of {stride} words and names word {matrix_start}",
                            b.resource.0
                        ));
                    }
                    if let BufferKind::Structured { stride: bytes } = buffer
                        && bytes != u32::from(*stride) * 4
                    {
                        return Err(format!(
                            "buffer slot {} holds instance records of {stride} words and has elements of {bytes} bytes",
                            b.resource.0
                        ));
                    }
                    let is_matrices = |other: &BufferBinding| {
                        other.resource == *matrices
                            && matches!(
                                other.source,
                                BufferSource::Table(
                                    BufferTable::ObjectMatrices { .. }
                                        | BufferTable::HeaderedMatrices { .. }
                                        | BufferTable::SkinnedMatrices { .. }
                                )
                            )
                    };
                    if !self.buffers.iter().any(is_matrices) {
                        return Err(format!(
                            "buffer slot {} names slot {} for its matrices, which is no matrix table of this stage",
                            b.resource.0, matrices.0
                        ));
                    }
                }
            }
        }
        if !self.buffers.is_empty()
            && let Some(r) = interface.resources.iter().find(|r| {
                matches!(r.kind, ResourceKind::Buffer(_)) && !self.buffers.iter().any(|b| b.resource == r.id)
            })
        {
            return Err(format!(
                "buffer slot {} has no entry: a stage that states buffers states every one (a table, or unknown with \
                 why)",
                r.id.0
            ));
        }
        Ok(())
    }
}
impl Component for ShaderBinding {
    const NAME: &'static str = "core.shader_binding";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, w: &crate::World) -> Result<(), String> {
        for (i, s) in self.stages.iter().enumerate() {
            if let Some(k) = &s.variant {
                if k.field.is_empty() {
                    return Err(format!("the {:?} stage's variant key names no field", s.stage));
                }
                if let Some(p) = k.parts.iter().enumerate().find_map(|(n, p)| {
                    k.parts[..n]
                        .iter()
                        .any(|before| before.name == p.name)
                        .then_some(&p.name)
                }) {
                    return Err(format!(
                        "the variant key {} = {} describes {p:?} twice",
                        k.field, k.value
                    ));
                }
                // the stages of one variant describe it the same way
                if let Some(other) = self.stages[..i].iter().find(|o| {
                    o.variant
                        .as_ref()
                        .is_some_and(|b| b.field == k.field && b.value == k.value && b.parts != k.parts)
                }) {
                    return Err(format!(
                        "the {:?} and {:?} stages of {} = {} describe it differently",
                        other.stage, s.stage, k.field, k.value
                    ));
                }
            }
            // a stage kind may repeat only under distinct values of one key
            for other in self.stages[..i].iter().filter(|o| o.stage == s.stage) {
                let distinct = match (&other.variant, &s.variant) {
                    (Some(a), Some(b)) => a.field == b.field && a.value != b.value,
                    _ => false,
                };
                if !distinct {
                    return Err(match &s.variant {
                        None => format!("the {:?} stage is bound twice", s.stage),
                        Some(k) => format!(
                            "the {:?} stage is bound twice for {} = {} (several need distinct values of one key)",
                            s.stage, k.field, k.value
                        ),
                    });
                }
            }
            s.verify_in(w, BindingPlace::Material)?;
        }
        self.state.verify()
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for s in &self.stages {
            out.push(s.shader);
            for t in &s.textures {
                if let TextureSource::Texture { texture, .. } = &t.source {
                    out.push(*texture);
                }
            }
        }
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use crate::shader::Attribute;
    use crate::shader::{InputRole, KeyPart, Occluded, ScalarEncoding, SkinPrefix, ViewParameter};
    use iridia_interface::AddressMode;
    use iridia_interface::{
        BufferLayout, Constant, ConstantType, ResourceProvenance, SampleType, ScalarType, ShaderFormat,
        TextureDim, Value as Lanes, Varying, VaryingProvenance,
    };
    use iridia_interface::{Resource, ResourceKind as Kind};
    fn varying(name: &str, width: u8) -> Varying {
        Varying {
            label: Label {
                name: name.into(),
                index: 0,
            },
            system: None,
            value: Lanes {
                scalar: ScalarType::Float,
                width,
            },
            interp: None,
            used: true,
            provenance: VaryingProvenance::default(),
        }
    }
    fn resource(id: u32, kind: Kind) -> Resource {
        Resource {
            id: ResourceId(id),
            kind,
            count: 1,
            provenance: ResourceProvenance::default(),
        }
    }
    /// A pixel shader with a constant buffer (0), a texture (1) and a sampler (2).
    fn shader() -> Shader {
        Shader {
            format: ShaderFormat::Dxbc,
            bytecode: vec![0; 8],
            interface: Interface {
                stage: Stage::Pixel,
                inputs: vec![varying("TEXCOORD", 2)],
                outputs: vec![varying("SV_Target", 4)],
                resources: vec![
                    resource(
                        0,
                        Kind::ConstantBuffer {
                            layout: BufferLayout {
                                size: 32,
                                constants: vec![Constant {
                                    name: "tint".into(),
                                    offset: 0,
                                    ty: ConstantType::Vector(Lanes {
                                        scalar: ScalarType::Float,
                                        width: 4,
                                    }),
                                    default: None,
                                }],
                            },
                        },
                    ),
                    resource(
                        1,
                        Kind::Texture {
                            dim: TextureDim::D2,
                            arrayed: false,
                            multisampled: false,
                            sample: SampleType::Float,
                        },
                    ),
                    resource(2, Kind::Sampler { comparison: false }),
                ],
                facts: Default::default(),
            },
        }
    }
    fn binding(shader: Entity, texture: Entity) -> ShaderBinding {
        ShaderBinding {
            stages: vec![StageBinding {
                stage: Stage::Pixel,
                variant: None,
                shader,
                inputs: vec![InputBinding {
                    label: Label {
                        name: "TEXCOORD".into(),
                        index: 0,
                    },
                    role: InputRole::Stream(Attribute::TexCoord(0)),
                }],
                constants: vec![
                    ConstantBinding {
                        resource: ResourceId(0),
                        offset: 0,
                        size: 16,
                        name: Some("tint".into()),
                        role: ConstantRole::Material(vec![1.0f32.to_bits(); 4]),
                        lines: None,
                    },
                    ConstantBinding {
                        resource: ResourceId(0),
                        offset: 16,
                        size: 16,
                        name: None,
                        role: ConstantRole::Unknown {
                            why: "cb0[1] is not in the recipe".into(),
                        },
                        lines: None,
                    },
                ],
                textures: vec![TextureBinding {
                    resource: ResourceId(1),
                    sampler: Some(ResourceId(2)),
                    source: TextureSource::Texture {
                        texture,
                        sampler: SamplerState {
                            wrap: [Some(AddressMode::Repeat); 3],
                            ..SamplerState::default()
                        },
                    },
                }],
                buffers: Vec::new(),
                constant_buffers: Vec::new(),
                outputs: vec![
                    OutputBinding {
                        label: Label {
                            name: "SV_Target".into(),
                            index: 0,
                        },
                        lanes: 0b0111,
                        role: OutputRole::Albedo,
                    },
                    OutputBinding {
                        label: Label {
                            name: "SV_Target".into(),
                            index: 0,
                        },
                        lanes: 0b1000,
                        role: OutputRole::Occlusion {
                            of: Occluded::AllLight,
                            encoding: ScalarEncoding::LINEAR,
                        },
                    },
                ],
            }],
            state: RenderState::default(),
        }
    }
    /// A vertex shader with a constant buffer (0) and structured buffers of 16 (10), 8 (11) and 12 (12) bytes an element.
    fn buffer_shader() -> Shader {
        let structured = |id, stride| {
            resource(
                id,
                Kind::Buffer(iridia_interface::BufferKind::Structured { stride }),
            )
        };
        Shader {
            format: ShaderFormat::Dxbc,
            bytecode: vec![0; 8],
            interface: Interface {
                stage: Stage::Vertex,
                inputs: Vec::new(),
                outputs: Vec::new(),
                resources: vec![
                    resource(
                        0,
                        Kind::ConstantBuffer {
                            layout: BufferLayout {
                                size: 16,
                                constants: Vec::new(),
                            },
                        },
                    ),
                    structured(10, 16),
                    structured(11, 8),
                    structured(12, 12),
                ],
                facts: Default::default(),
            },
        }
    }
    fn buffer_binding(shader: Entity, buffers: Vec<BufferBinding>) -> ShaderBinding {
        ShaderBinding {
            stages: vec![StageBinding {
                stage: Stage::Vertex,
                variant: None,
                shader,
                inputs: Vec::new(),
                constants: Vec::new(),
                textures: Vec::new(),
                buffers,
                constant_buffers: Vec::new(),
                outputs: Vec::new(),
            }],
            state: RenderState::default(),
        }
    }
    fn matrices(resource: u32, rows: u8) -> BufferBinding {
        BufferBinding {
            resource: ResourceId(resource),
            source: BufferSource::Table(BufferTable::ObjectMatrices { rows }),
        }
    }
    fn headered(resource: u32, rows: u8) -> BufferBinding {
        BufferBinding {
            resource: ResourceId(resource),
            source: BufferSource::Table(BufferTable::HeaderedMatrices { rows }),
        }
    }
    fn skinned(resource: u32, rows: u8, joint_stride: u8, prefix: u8) -> BufferBinding {
        BufferBinding {
            resource: ResourceId(resource),
            source: BufferSource::Table(BufferTable::SkinnedMatrices {
                rows,
                joint_stride,
                prefix,
                first: SkinPrefix::Translation,
            }),
        }
    }
    fn instances(resource: u32, stride: u8, matrix_start: u8, table: u32) -> BufferBinding {
        BufferBinding {
            resource: ResourceId(resource),
            source: BufferSource::Table(BufferTable::Instances {
                stride,
                matrix_start,
                matrices: ResourceId(table),
            }),
        }
    }
    /// A render graph's buffer is a buffer only a step of the graph binds: a material's binding that states one is refused,
    /// and the same stage verifies in a graph step's place.
    #[test]
    fn a_material_binds_no_render_graph_buffer() {
        let mut w = World::new();
        let (s, m) = (w.spawn(), w.spawn());
        w.insert(s, buffer_shader());
        let graph = |resource: u32| BufferBinding {
            resource: ResourceId(resource),
            source: BufferSource::Graph(0),
        };
        let b = buffer_binding(s, vec![graph(10), graph(11), graph(12)]);
        w.insert(m, b.clone());
        let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
        assert!(
            err.contains(
                "buffer slot 10 binds a render graph buffer, which only a step of a core.render_graph may"
            ),
            "{err}"
        );
        assert_eq!(b.stages[0].verify_in(&w, BindingPlace::GraphStep), Ok(()));
    }
    /// The buffer slots: a matrix table and the instance records that index it verify, each rule broken once is named,
    /// and the roles serialize under their own names.
    #[test]
    fn buffer_slots_name_the_tables_the_program_indexes() {
        let mut w = World::new();
        let (s, m) = (w.spawn(), w.spawn());
        w.insert(s, buffer_shader());
        // every buffer named: 12 is unknown with a reason
        let rest = || BufferBinding {
            resource: ResourceId(12),
            source: BufferSource::Unknown {
                why: "not read here".into(),
            },
        };
        let good = vec![matrices(10, 3), instances(11, 2, 0, 10), rest()];
        w.insert(m, buffer_binding(s, good.clone()));
        assert_eq!(w.get::<ShaderBinding>(m).unwrap().verify(m, &w), Ok(()));
        let json = w.get::<ShaderBinding>(m).unwrap().to_json().unwrap().unwrap();
        assert_eq!(
            json["stages"][0]["buffers"][0]["source"],
            serde_json::json!({"table": {"object_matrices": {"rows": 3}}})
        );
        assert_eq!(
            json["stages"][0]["buffers"][1]["source"],
            serde_json::json!({"table": {"instances": {"stride": 2, "matrix_start": 0, "matrices": 10}}})
        );
        let unknown = |why: &str| BufferBinding {
            resource: ResourceId(12),
            source: BufferSource::Unknown { why: why.into() },
        };
        let cases: Vec<(Vec<BufferBinding>, &str)> = vec![
            (vec![matrices(0, 3)], "is not a buffer"),
            (vec![matrices(99, 3)], "is not a buffer"),
            (vec![matrices(10, 3), matrices(10, 4)], "bound twice"),
            (vec![matrices(10, 5)], "not 3 or 4"),
            (vec![matrices(11, 3)], "has elements of 8"),
            (vec![matrices(10, 3), instances(11, 2, 2, 10)], "names word 2"),
            (vec![matrices(10, 3), instances(11, 0, 0, 10)], "names word 0"),
            (
                vec![matrices(10, 3), instances(11, 3, 0, 10)],
                "has elements of 8 bytes",
            ),
            (
                vec![matrices(10, 3), instances(11, 2, 0, 12)],
                "no matrix table of this stage",
            ),
            (vec![instances(11, 2, 0, 10)], "no matrix table of this stage"),
            (
                vec![matrices(10, 3), instances(11, 2, 0, 11)],
                "no matrix table of this stage",
            ),
            (vec![unknown("")], "says not why"),
        ];
        for (buffers, message) in cases {
            w.insert(m, buffer_binding(s, buffers.clone()));
            let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
            assert!(err.contains(message), "{buffers:?}: {err}");
        }
        // an unknown buffer with a reason, beside a table, is a binding
        w.insert(
            m,
            buffer_binding(
                s,
                vec![
                    matrices(10, 4),
                    instances(11, 2, 0, 10),
                    unknown("the engine fills it"),
                ],
            ),
        );
        assert_eq!(w.get::<ShaderBinding>(m).unwrap().verify(m, &w), Ok(()));
        // a headered table is a matrix table: records may name it, and it has the same rows and element rules
        w.insert(
            m,
            buffer_binding(s, vec![headered(10, 3), instances(11, 2, 1, 10), rest()]),
        );
        assert_eq!(w.get::<ShaderBinding>(m).unwrap().verify(m, &w), Ok(()));
        let json = w.get::<ShaderBinding>(m).unwrap().to_json().unwrap().unwrap();
        assert_eq!(
            json["stages"][0]["buffers"][0]["source"],
            serde_json::json!({"table": {"headered_matrices": {"rows": 3}}})
        );
        // a skinned block is a matrix table too, with the joints no closer than their rows and a prefix to describe
        w.insert(
            m,
            buffer_binding(s, vec![skinned(10, 3, 3, 3), instances(11, 2, 1, 10), rest()]),
        );
        assert_eq!(w.get::<ShaderBinding>(m).unwrap().verify(m, &w), Ok(()));
        let json = w.get::<ShaderBinding>(m).unwrap().to_json().unwrap().unwrap();
        assert_eq!(
            json["stages"][0]["buffers"][0]["source"],
            serde_json::json!({"table": {"skinned_matrices": {
                "rows": 3, "joint_stride": 3, "prefix": 3, "first": "translation"
            }}})
        );
        for (buffers, message) in [
            (
                vec![skinned(10, 3, 2, 3), instances(11, 2, 1, 10), rest()],
                "elements apart with 3 rows",
            ),
            (
                vec![skinned(10, 3, 3, 0), instances(11, 2, 1, 10), rest()],
                "no prefix element to say what the first one holds",
            ),
            (
                vec![skinned(10, 5, 5, 3), instances(11, 2, 1, 10), rest()],
                "not 3 or 4",
            ),
        ] {
            w.insert(m, buffer_binding(s, buffers.clone()));
            let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
            assert!(err.contains(message), "{buffers:?}: {err}");
            // no message holds a run of spaces (a line continuation once ate a newline inside one)
            assert!(!err.contains("  "), "{err}");
        }
        // a stage that states buffers states every one; one that states none leaves each unknown with that reason
        w.insert(
            m,
            buffer_binding(s, vec![matrices(10, 3), instances(11, 2, 0, 10)]),
        );
        let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
        assert!(err.contains("buffer slot 12 has no entry"), "{err}");
        w.insert(m, buffer_binding(s, Vec::new()));
        let binding = w.get::<ShaderBinding>(m).unwrap();
        assert_eq!(binding.verify(m, &w), Ok(()));
        assert_eq!(
            binding.stages[0].buffer(ResourceId(10)),
            BufferSource::Unknown {
                why: "the source states no buffers for this stage".into()
            }
        );
        w.insert(m, buffer_binding(s, good.clone()));
        assert_eq!(
            w.get::<ShaderBinding>(m).unwrap().stages[0].buffer(ResourceId(10)),
            BufferSource::Table(BufferTable::ObjectMatrices { rows: 3 })
        );
        for (buffers, message) in [
            (vec![headered(10, 5)], "not 3 or 4"),
            (vec![headered(11, 3)], "has elements of 8"),
            (vec![headered(10, 3), matrices(10, 3)], "bound twice"),
            (vec![headered(0, 3)], "is not a buffer"),
        ] {
            w.insert(m, buffer_binding(s, buffers.clone()));
            let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
            assert!(err.contains(message), "{buffers:?}: {err}");
        }
    }
    /// The constant buffers a source binds larger than the program declares: a constant buffer of the shader, once, in whole
    /// rows and no smaller than the declaration (16 bytes here); `None` is the declared size.
    #[test]
    fn a_constant_buffer_is_bound_at_a_whole_number_of_rows_no_smaller_than_declared() {
        let mut w = World::new();
        let (s, m) = (w.spawn(), w.spawn());
        w.insert(s, buffer_shader());
        let bound = |resource: u32, bound_size| ConstantBufferBinding {
            resource: ResourceId(resource),
            bound_size,
        };
        let with = |constant_buffers: Vec<ConstantBufferBinding>| {
            let mut b = buffer_binding(s, Vec::new());
            b.stages[0].constant_buffers = constant_buffers;
            b
        };
        w.insert(m, with(vec![bound(0, Some(12_288))]));
        let binding = w.get::<ShaderBinding>(m).unwrap();
        assert_eq!(binding.verify(m, &w), Ok(()));
        assert_eq!(binding.stages[0].bound_size(ResourceId(0)), Some(12_288));
        let json = binding.to_json().unwrap().unwrap();
        assert_eq!(
            json["stages"][0]["constant_buffers"][0],
            serde_json::json!({"resource": 0, "bound_size": 12288})
        );
        // the joint palette constant's matrices: its size, or the bound size where the source states one; none when it is
        // held as columns
        let palette = |lines| ConstantBinding {
            resource: ResourceId(0),
            offset: 0,
            size: 96,
            name: None,
            role: ConstantRole::Object(ObjectParameter::BonePalette),
            lines,
        };
        let mut stage = binding.stages[0].clone();
        stage.constants = vec![palette(Some(MatrixLines::Rows))];
        assert_eq!(stage.bone_palette_matrices(), Some(256));
        stage.constant_buffers.clear();
        assert_eq!(stage.bone_palette_matrices(), Some(2));
        stage.constants = vec![palette(Some(MatrixLines::Columns))];
        assert_eq!(stage.bone_palette_matrices(), None);
        stage.constants.clear();
        assert_eq!(stage.bone_palette_matrices(), None);
        // the declared size, and a buffer with no entry
        w.insert(m, with(vec![bound(0, None)]));
        let binding = w.get::<ShaderBinding>(m).unwrap();
        assert_eq!(binding.verify(m, &w), Ok(()));
        assert_eq!(binding.stages[0].bound_size(ResourceId(0)), None);
        assert_eq!(binding.stages[0].bound_size(ResourceId(10)), None);
        for (buffers, message) in [
            (vec![bound(10, Some(64))], "is not a constant buffer"),
            (vec![bound(99, Some(64))], "is not a constant buffer"),
            (vec![bound(0, Some(32)), bound(0, None)], "bound twice"),
            (vec![bound(0, Some(40))], "not a whole number of 16-byte rows"),
            (
                vec![bound(0, Some(0))],
                "smaller than the 16 the program declares",
            ),
        ] {
            w.insert(m, with(buffers.clone()));
            let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
            assert!(err.contains(message), "{buffers:?}: {err}");
        }
    }
    /// A world with a shader, a texture and a material carrying the binding built from them.
    fn world() -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let (s, t, m) = (w.spawn(), w.spawn(), w.spawn());
        w.insert(s, shader());
        w.insert(
            t,
            TextureFile {
                path: "albedo.png".into(),
            },
        );
        w.insert(m, binding(s, t));
        (w, s, t, m)
    }
    /// An output's encoding of power or root 0, or of a scale that isn't finite, is refused, naming the output.
    #[test]
    fn an_encoding_of_no_power_no_root_or_no_finite_scale_is_refused() {
        let one = 1f32.to_bits();
        for bad in [
            ScalarEncoding {
                power: 0,
                root: 1,
                scale_bits: one,
            },
            ScalarEncoding {
                power: 1,
                root: 0,
                scale_bits: one,
            },
            ScalarEncoding {
                power: 1,
                root: 1,
                scale_bits: f32::INFINITY.to_bits(),
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
            let (mut w, _, _, m) = world();
            let mut b = w.get::<ShaderBinding>(m).unwrap().clone();
            b.stages[0].outputs[1].role = OutputRole::Occlusion {
                of: Occluded::AllLight,
                encoding: bad,
            };
            w.insert(m, b);
            let err = w.get::<ShaderBinding>(m).unwrap().verify(m, &w).unwrap_err();
            assert!(err.contains("output SV_Target0: an encoding of"), "{err}");
        }
        assert_eq!(ScalarEncoding::square_of(0.95).check(), Ok(()));
        assert_eq!(ScalarEncoding::LINEAR.check(), Ok(()));
    }
    /// A matrix role's binding states how its lines hold the matrix, and no other role's does: either missing or
    /// misplaced is refused, naming the constant.
    #[test]
    fn a_matrix_role_states_its_lines_and_no_other_role_does() {
        let (mut w, _, _, m) = world();
        let verify = |w: &mut World, role: ConstantRole, lines: Option<MatrixLines>| {
            let mut b = w.get::<ShaderBinding>(m).unwrap().clone();
            b.stages[0].constants[0].role = role;
            b.stages[0].constants[0].lines = lines;
            w.insert(m, b);
            w.get::<ShaderBinding>(m).unwrap().verify(m, w)
        };
        let view_projection = ConstantRole::View(ViewParameter::ViewProjection);
        let err = verify(&mut w, view_projection.clone(), None).unwrap_err();
        assert!(
            err.contains("constant tint at 0 is a matrix") && err.contains("lines are not stated"),
            "{err}"
        );
        let unexamined = |why: &str| MatrixLines::Unshown(Unshown::Unexamined { why: why.into() });
        for lines in [
            MatrixLines::Rows,
            MatrixLines::Columns,
            MatrixLines::Unshown(Unshown::NotApplied),
            unexamined("no value graph"),
        ] {
            assert_eq!(verify(&mut w, view_projection.clone(), Some(lines)), Ok(()));
        }
        let err = verify(&mut w, view_projection.clone(), Some(unexamined(" "))).unwrap_err();
        assert!(err.contains("unshown with no reason"), "{err}");
        assert_eq!(
            verify(
                &mut w,
                ConstantRole::Object(ObjectParameter::BonePalette),
                Some(MatrixLines::Rows)
            ),
            Ok(())
        );
        let err = verify(
            &mut w,
            ConstantRole::Material(vec![0; 4]),
            Some(MatrixLines::Rows),
        )
        .unwrap_err();
        assert!(
            err.contains("states lines (Rows) for a role that is no matrix"),
            "{err}"
        );
        assert!(!ConstantRole::Object(ObjectParameter::TextureTransformRow(0)).is_matrix());
    }
    #[test]
    fn a_binding_names_what_it_refers_to_and_verifies() {
        let (w, s, t, m) = world();
        let b = w.get::<ShaderBinding>(m).unwrap();
        assert_eq!(b.verify(m, &w), Ok(()));
        let mut refs = Vec::new();
        b.references(&mut refs);
        assert_eq!(refs, [s, t]);
        let json = b.to_json().unwrap().unwrap();
        assert_eq!(json["stages"][0]["stage"], "pixel");
        // the role names what it occludes and how it is stored, the scale as a number
        assert_eq!(
            json["stages"][0]["outputs"][1]["role"],
            serde_json::json!({
                "occlusion": {"of": "all_light", "encoding": {"power": 1, "scale": 1.0}}
            })
        );
        assert_eq!(
            json["stages"][0]["constants"][1]["role"]["unknown"]["why"],
            "cb0[1] is not in the recipe"
        );
        assert!(b.stage(Stage::Vertex).is_none() && b.stage(Stage::Pixel).is_some());
    }
    /// Each rule of `verify`, broken once.
    #[test]
    fn each_broken_reference_is_named() {
        let (w, _, _, m) = world();
        let good = w.get::<ShaderBinding>(m).unwrap().clone();
        type Break = Box<dyn Fn(&mut ShaderBinding)>;
        let cases: Vec<(Break, &str)> = vec![
            (Box::new(|b| b.stages.push(b.stages[0].clone())), "bound twice"),
            (
                Box::new(|b| b.stages[0].inputs[0].label.index = 7),
                "input TEXCOORD7",
            ),
            (Box::new(|b| b.stages[0].outputs[0].lanes = 0b10000), "lanes"),
            (
                Box::new(|b| b.stages[0].constants[0].resource = ResourceId(1)),
                "not a constant buffer",
            ),
            (
                Box::new(|b| b.stages[0].constants[1].offset = 24),
                "outside buffer",
            ),
            (
                Box::new(|b| b.stages[0].constants[0].role = ConstantRole::Material(vec![0; 5])),
                "material words",
            ),
            (
                Box::new(|b| b.stages[0].textures[0].resource = ResourceId(0)),
                "not a texture",
            ),
            (
                Box::new(|b| b.stages[0].textures[0].sampler = Some(ResourceId(1))),
                "not a sampler",
            ),
            (
                Box::new(|b| {
                    let first = b.stages[0].inputs[0].clone();
                    b.stages[0].inputs.push(first);
                }),
                "input TEXCOORD0 is bound twice",
            ),
            (
                // the fixture's two roles on SV_Target0 are lanes 0b0111 and 0b1000: widening the second overlaps
                Box::new(|b| b.stages[0].outputs[1].lanes = 0b1100),
                "two roles on lanes 0x4",
            ),
            (
                Box::new(|b| b.stages[0].constants[1].offset = 8),
                "constants overlap in buffer 0: bytes 0..16 and 8..24",
            ),
            (
                Box::new(|b| {
                    let first = b.stages[0].textures[0].clone();
                    b.stages[0].textures.push(first);
                }),
                "texture slot 1 is bound twice",
            ),
            (
                Box::new(|b| {
                    b.stages[0].textures[0].source = TextureSource::Frame(FrameTexture::ReflectionProbe)
                }),
                "binds the frame's ReflectionProbe, a Cube image",
            ),
        ];
        for (break_it, expect) in cases {
            let mut b = good.clone();
            break_it(&mut b);
            let message = b.verify(m, &w).unwrap_err();
            assert!(message.contains(expect), "{expect}: {message}");
        }
        // an entity that is not a shader, a shader of another stage, a texture slot that binds a non-texture
        let mut b = good.clone();
        b.stages[0].shader = m;
        assert!(b.verify(m, &w).unwrap_err().contains("no core.shader"));
        let mut b = good.clone();
        b.stages[0].stage = Stage::Vertex;
        assert!(b.verify(m, &w).unwrap_err().contains("Pixel program"));
        let mut b = good;
        if let TextureSource::Texture { texture, .. } = &mut b.stages[0].textures[0].source {
            *texture = m;
        }
        assert!(b.verify(m, &w).unwrap_err().contains("neither core.texture"));
    }
    /// A pixel program's written lanes each have a binding: lanes its D3D signature writes and no binding states are
    /// refused, but not the lanes it never writes, nor a system value other than a target, nor a vertex program's
    /// outputs (the next stage's inputs); an Unknown output states why.
    #[test]
    fn a_pixel_programs_written_lanes_each_have_a_binding() {
        let (mut w, s, _, m) = world();
        let signature = |w: &mut World, read_write_mask: u8, system: Option<SystemValue>, stage: Stage| {
            let mut shader = w.get::<Shader>(s).unwrap().clone();
            shader.interface.stage = stage;
            let out = &mut shader.interface.outputs[0];
            out.system = system;
            out.provenance.d3d = Some(iridia_interface::D3dSignature {
                register: 0,
                mask: 0b1111,
                read_write_mask,
            });
            w.insert(s, shader);
        };
        let verify = |w: &World, b: &ShaderBinding| b.verify(m, w);
        let good = w.get::<ShaderBinding>(m).unwrap().clone();
        // the fixture binds lanes 0b0111 and 0b1000 of SV_Target0, all four written
        signature(&mut w, 0, None, Stage::Pixel);
        assert_eq!(verify(&w, &good), Ok(()));
        let mut rgb = good.clone();
        rgb.stages[0].outputs.truncate(1);
        assert_eq!(
            rgb.stages[0].unbound_written_lanes(&w.get::<Shader>(s).unwrap().interface),
            [(&good.stages[0].outputs[0].label, 0b1000)]
        );
        let err = verify(&w, &rgb).unwrap_err();
        assert!(
            err.contains("output SV_Target0 lanes 0b1000 are written and have no binding"),
            "{err}"
        );
        // the program never writes w
        signature(&mut w, 0b1000, None, Stage::Pixel);
        assert_eq!(verify(&w, &rgb), Ok(()));
        // a target stated as a system value is held the same; a depth is not a target
        signature(&mut w, 0, Some(SystemValue::Target(0)), Stage::Pixel);
        assert!(verify(&w, &rgb).is_err());
        signature(&mut w, 0, Some(SystemValue::Depth), Stage::Pixel);
        assert_eq!(verify(&w, &rgb), Ok(()));
        // a vertex program's outputs are bound as the pixel program's inputs
        signature(&mut w, 0, None, Stage::Vertex);
        let mut vertex = rgb.clone();
        vertex.stages[0].stage = Stage::Vertex;
        assert_eq!(verify(&w, &vertex), Ok(()));
        // the w lane Unknown, with why and without
        signature(&mut w, 0, None, Stage::Pixel);
        let mut unknown = good;
        unknown.stages[0].outputs[1].role = OutputRole::unknown("read by nothing examined");
        assert_eq!(verify(&w, &unknown), Ok(()));
        unknown.stages[0].outputs[1].role = OutputRole::unknown(" ");
        let err = verify(&w, &unknown).unwrap_err();
        assert!(err.contains("output SV_Target0 is Unknown with no why"), "{err}");
    }
    /// A frame image on a slot of its own dimension, and a constant colour on any, verify.
    #[test]
    fn a_frame_image_fits_the_slot_it_is_bound_to() {
        let (w, _, _, m) = world();
        let mut b = w.get::<ShaderBinding>(m).unwrap().clone();
        b.stages[0].textures[0].source = TextureSource::Frame(FrameTexture::SceneColour);
        assert_eq!(b.verify(m, &w), Ok(()));
        b.stages[0].textures[0].source = TextureSource::Default {
            value: [0.5; 4],
            why: "a constant".into(),
        };
        assert_eq!(b.verify(m, &w), Ok(()));
        assert_eq!(FrameTexture::LightProbeVolume.dim(), TextureDim::D3);
        assert_eq!(FrameTexture::ShadowMap.dim(), TextureDim::D2);
    }
    /// A render graph's target is a texture only a step of the graph binds: a material's binding that states one is
    /// refused, and the same stage verifies in a graph step's place.
    #[test]
    fn a_material_binds_no_render_graph_target() {
        let (w, _, _, m) = world();
        let mut b = w.get::<ShaderBinding>(m).unwrap().clone();
        b.stages[0].textures[0].source = TextureSource::Target {
            view: crate::render_graph::TargetView {
                target: 0,
                mip: 0,
                first_layer: 0,
                layers: 0,
            },
            sampler: SamplerState::default(),
        };
        let err = b.verify(m, &w).unwrap_err();
        assert!(
            err.contains("binds a render graph target, which only a step of a core.render_graph may"),
            "{err}"
        );
        assert_eq!(b.stages[0].verify_in(&w, BindingPlace::GraphStep), Ok(()));
    }
    /// A texture of several images fits a slot read as the dimension its shape is, and no other.
    #[test]
    fn a_layered_texture_fits_the_slot_of_its_shape() {
        use crate::{TexFormat, TextureLayers, TextureShape};
        let layers = |shape: TextureShape| TextureLayers {
            shape,
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![(0..shape.layers_at(0)).map(|_| vec![0; 4]).collect()],
            color_space: crate::ColorSpace::Linear,
            role: crate::TexRole::Other,
            normal: None,
        };
        let (mut w, s, t, m) = world();
        let b = w.get::<ShaderBinding>(m).unwrap().clone();
        w.remove::<TextureFile>(t);
        // the fixture's slot is a plain 2D texture: no layered shape is that
        for shape in [
            TextureShape::Array { layers: 2 },
            TextureShape::Cube { cubes: 1 },
            TextureShape::Volume { depth: 1 },
        ] {
            w.insert(t, layers(shape));
            let why = b.verify(m, &w).unwrap_err();
            assert!(
                why.contains("is a D2 resource (arrayed false) and binds"),
                "{why}"
            );
        }
        // the slot read as an arrayed 2D texture takes an array and nothing else
        let Kind::Texture { arrayed, .. } = &mut w.get_mut::<Shader>(s).unwrap().interface.resources[1].kind
        else {
            panic!("the fixture's second resource is a texture");
        };
        *arrayed = true;
        w.insert(t, layers(TextureShape::Array { layers: 3 }));
        assert_eq!(b.verify(m, &w), Ok(()));
        w.insert(t, layers(TextureShape::Cube { cubes: 2 }));
        assert!(b.verify(m, &w).is_err());
    }
    /// A variant key names a field, describes each part once, and describes one variant the same way on all its stages.
    #[test]
    fn a_variant_keys_description_is_consistent() {
        let (w, _, _, m) = world();
        let part = |name: &str, value: u64| KeyPart {
            name: name.into(),
            value,
        };
        let key = |field: &str, value: u32, parts: Vec<KeyPart>| {
            Some(VariantKey {
                field: field.into(),
                value,
                parts,
            })
        };
        let good = w.get::<ShaderBinding>(m).unwrap().clone();
        let with = |a: Option<VariantKey>| {
            let mut b = good.clone();
            b.stages[0].variant = a;
            b
        };
        assert_eq!(
            with(key("f.variant", 0, vec![part("program", 1), part("outputs", 5)])).verify(m, &w),
            Ok(())
        );
        let message = with(key("", 0, Vec::new())).verify(m, &w).unwrap_err();
        assert!(message.contains("names no field"), "{message}");
        let message = with(key("f.variant", 0, vec![part("program", 1), part("program", 2)]))
            .verify(m, &w)
            .unwrap_err();
        assert!(message.contains("describes \"program\" twice"), "{message}");
        // two stages of one variant (a vertex and a pixel stage share the key) must describe it alike; the pixel
        // fixture stands for both, a second stage of another kind is built from it
        let mut b = with(key("f.variant", 3, vec![part("program", 1)]));
        let mut other = b.stages[0].clone();
        other.stage = Stage::Vertex;
        other.variant = key("f.variant", 3, vec![part("program", 2)]);
        b.stages.push(other);
        let message = b.verify(m, &w).unwrap_err();
        assert!(message.contains("describe it differently"), "{message}");
    }
    /// A stage kind repeats only under distinct values of one key: two pixel stages with keys `k = 1` and `k = 2` verify,
    /// the same value twice, a key and no key, and two different fields do not.
    #[test]
    fn a_stage_repeats_only_under_distinct_values_of_one_key() {
        let (w, _, _, m) = world();
        let good = w.get::<ShaderBinding>(m).unwrap().clone();
        let key = |field: &str, value: u32| {
            Some(VariantKey {
                field: field.to_owned(),
                value,
                parts: Vec::new(),
            })
        };
        let with = |a: Option<VariantKey>, b: Option<VariantKey>| {
            let mut x = good.clone();
            x.stages[0].variant = a;
            let mut y = x.stages[0].clone();
            y.variant = b;
            x.stages.push(y);
            x
        };
        assert_eq!(
            with(key("g.mesh.kind", 1), key("g.mesh.kind", 2)).verify(m, &w),
            Ok(())
        );
        let same = with(key("g.mesh.kind", 1), key("g.mesh.kind", 1))
            .verify(m, &w)
            .unwrap_err();
        assert!(same.contains("g.mesh.kind = 1"), "{same}");
        assert!(with(key("g.mesh.kind", 1), None).verify(m, &w).is_err());
        assert!(with(None, key("g.mesh.kind", 1)).verify(m, &w).is_err());
        assert!(
            with(None, None)
                .verify(m, &w)
                .unwrap_err()
                .contains("bound twice")
        );
        assert!(
            with(key("g.mesh.kind", 1), key("g.mesh.other", 2))
                .verify(m, &w)
                .is_err()
        );
        // a third stage repeating the first one's value is refused though it differs from the second's (it is compared
        // with every earlier stage of its kind)
        let mut three = with(key("g.mesh.kind", 1), key("g.mesh.kind", 2));
        let mut z = three.stages[0].clone();
        z.variant = key("g.mesh.kind", 1);
        three.stages.push(z);
        assert!(three.verify(m, &w).is_err());
    }
}
