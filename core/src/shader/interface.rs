//! A program's interface: the varyings it reads and writes, and the system values among them.

use serde::Serialize;

use super::{BufferKind, Resource, ResourceId, ResourceKind, Stage, StorageTarget};

/// The scalar type of a value or constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    /// A boolean, 32 bits wide wherever it is stored (constant buffers, signatures).
    Bool,
    Sint,
    Uint,
    Float,
    Double,
}
impl ScalarType {
    /// Bytes one scalar takes in a constant buffer.
    pub fn bytes(self) -> u32 {
        match self {
            Self::Double => 8,
            _ => 4,
        }
    }
}

/// A scalar type and a width of 1 to 4 lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Value {
    pub scalar: ScalarType,
    pub width: u8,
}

/// A varying's name in its source: the semantic name and its index (DXBC `TEXCOORD` and 5, SPIR-V `Location` and 3,
/// GLSL `vUv` and 0). Labels are not interpreted: games put anything in `TEXCOORDn`, and what a label means is a
/// game resolve pass's business. Stages match by label.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Label {
    pub name: String,
    pub index: u32,
}

/// The system values, one vocabulary for every API: what the pipeline itself supplies to or takes from a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemValue {
    /// A vertex stage's clip-space position output.
    ClipPosition,
    /// A pixel stage's window-space position input (x, y, depth, 1/w).
    FragCoord,
    FrontFacing,
    VertexIndex,
    InstanceIndex,
    PrimitiveIndex,
    SampleIndex,
    RenderTargetArrayIndex,
    ViewportArrayIndex,
    /// Clip distances; the label's index says which array element.
    ClipDistance,
    /// Cull distances; the label's index says which array element.
    CullDistance,
    /// Colour output `n`.
    Target(u32),
    /// A pixel stage's depth output, written freely.
    Depth,
    /// Depth output the program promises is not less than the rasterized depth.
    DepthGreaterEqual,
    /// Depth output the program promises is not greater than the rasterized depth.
    DepthLessEqual,
    StencilRef,
    /// Coverage mask, as an input or an output.
    Coverage,
    /// Coverage of the pixel by the primitive alone (conservative rasterization).
    InnerCoverage,
}

/// How a varying is interpolated across a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpMode {
    /// Perspective-correct (the default for an interpolant).
    Perspective,
    /// Linear in screen space (HLSL `noperspective`).
    Linear,
    /// The provoking vertex's value, not interpolated.
    Flat,
}

/// Where in a pixel a varying is sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpSampling {
    Center,
    Centroid,
    /// Per sample.
    Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Interpolation {
    pub mode: InterpMode,
    pub sampling: InterpSampling,
}

/// Where a D3D signature element sits: its register (four 32-bit lanes each) and the lanes of it the element
/// occupies, as a mask (bit k is lane k, x first), and the signature's read/write mask as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct D3dSignature {
    pub register: u32,
    pub mask: u8,
    /// The signature's second mask as stored: for an input, the lanes the program always reads; for an output, the
    /// lanes it never writes.
    pub read_write_mask: u8,
}

/// The source's own address of a varying; one field per API that has a notion of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct VaryingProvenance {
    pub d3d: Option<D3dSignature>,
}

/// One interface variable: a value passed between stages (or between a stage and the pipeline), one per label. D3D
/// packs several into one register; they are separate here, with the register and lanes kept in provenance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Varying {
    /// The source's name for it, kept for system values too (`SV_Target` and 1).
    pub label: Label,
    /// What the pipeline itself supplies or takes here, when it is a system value.
    pub system: Option<SystemValue>,
    pub value: Value,
    /// How it is interpolated, where the source states it for this varying: a pixel input the program declares.
    /// `None` for outputs, for stages that do not interpolate, and for an input the program never declares (a source
    /// that carries interpolation in the program body leaves it out of a signature read alone).
    pub interp: Option<Interpolation>,
    /// Whether the program reads (an input) or writes (an output) at least one lane, as the source's signature
    /// states.
    pub used: bool,
    pub provenance: VaryingProvenance,
}

/// What a program exposes: its stage, the varyings it takes and gives, and the resources it binds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Interface {
    pub stage: Stage,
    pub inputs: Vec<Varying>,
    pub outputs: Vec<Varying>,
    pub resources: Vec<Resource>,
}
impl Interface {
    /// The resource with this id.
    pub fn resource(&self, id: ResourceId) -> Option<&Resource> {
        self.resources.iter().find(|r| r.id == id)
    }

    /// The interface's own invariants: one varying per label in each direction, values of 1 to 4 lanes, resource ids
    /// unique, every resource spanning at least one binding with a buffer stride or element width that means
    /// something, and every constant of a buffer inside it (with a layout that does not overflow).
    pub fn verify(&self) -> Result<(), String> {
        for (direction, varyings) in [("input", &self.inputs), ("output", &self.outputs)] {
            for (i, v) in varyings.iter().enumerate() {
                if !(1..=4).contains(&v.value.width) {
                    return Err(format!(
                        "{direction} {}{} has a width of {}, not 1 to 4",
                        v.label.name, v.label.index, v.value.width
                    ));
                }
                if varyings[..i].iter().any(|other| other.label == v.label) {
                    return Err(format!(
                        "{direction} {}{} is listed twice",
                        v.label.name, v.label.index
                    ));
                }
            }
        }
        let width_ok = |w: u8| (1..=4).contains(&w);
        for (i, r) in self.resources.iter().enumerate() {
            let name = &r.provenance.name;
            if self.resources[..i].iter().any(|other| other.id == r.id) {
                return Err(format!("resource id {} ({name}) is used twice", r.id.0));
            }
            if r.count == 0 {
                return Err(format!("resource {name} spans no bindings"));
            }
            let buffer = |kind: &BufferKind| match kind {
                BufferKind::Typed(v) if !width_ok(v.width) => {
                    Err(format!("buffer {name} has elements of {} lanes", v.width))
                }
                BufferKind::Structured { stride: 0 } => Err(format!("buffer {name} has a stride of 0")),
                _ => Ok(()),
            };
            match &r.kind {
                ResourceKind::Buffer(kind)
                | ResourceKind::Storage {
                    target: StorageTarget::Buffer(kind),
                    ..
                } => {
                    buffer(kind)?;
                }
                ResourceKind::ConstantBuffer { layout } => {
                    for c in &layout.constants {
                        let size =
                            c.ty.size()
                                .ok_or_else(|| format!("constant {name}.{} overflows", c.name))?;
                        if c.offset.checked_add(size).is_none_or(|end| end > layout.size) {
                            return Err(format!(
                                "constant {name}.{} ({size} bytes at {}) reaches past the end of the {}-byte buffer",
                                c.name, c.offset, layout.size
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_varying_serializes_with_its_provenance() {
        let v = Varying {
            label: Label {
                name: "TEXCOORD".into(),
                index: 5,
            },
            system: None,
            value: Value {
                scalar: ScalarType::Float,
                width: 2,
            },
            interp: Some(Interpolation {
                mode: InterpMode::Perspective,
                sampling: InterpSampling::Center,
            }),
            used: true,
            provenance: VaryingProvenance {
                d3d: Some(D3dSignature {
                    register: 3,
                    mask: 0b1100,
                    read_write_mask: 0b1100,
                }),
            },
        };
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({
                "label": {"name": "TEXCOORD", "index": 5},
                "system": null,
                "value": {"scalar": "float", "width": 2},
                "interp": {"mode": "perspective", "sampling": "center"},
                "used": true,
                "provenance": {"d3d": {"register": 3, "mask": 12, "read_write_mask": 12}},
            })
        );
        assert_eq!(
            serde_json::to_value(SystemValue::Target(2)).unwrap(),
            serde_json::json!({"target": 2})
        );
    }
}
