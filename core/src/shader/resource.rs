//! A program's resources: constant buffers with explicit layouts, textures, samplers, buffers and storage.

use serde::Serialize;

use super::{ScalarType, Value};

/// Names a resource within one [`super::Interface`]; the reader numbers them in its source's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ResourceId(pub u32);

/// The dimensionality of a texture or image. Whether it is an array or multisampled is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureDim {
    D1,
    D2,
    D3,
    Cube,
}

/// What a texture returns when it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleType {
    Float,
    Sint,
    Uint,
    /// A depth texture (a source whose API distinguishes it from a float texture; D3D does not).
    Depth,
}

/// How a buffer's elements are addressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferKind {
    /// Elements of one value type (a formatted view).
    Typed(Value),
    /// Elements of this many bytes.
    Structured { stride: u32 },
    /// Bytes, addressed by offset.
    Raw,
}

/// What a storage resource is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageTarget {
    Image {
        dim: TextureDim,
        arrayed: bool,
        sample: SampleType,
    },
    Buffer(BufferKind),
}

/// What a program may do to a storage resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

/// Whether consecutive lines of a matrix are its rows or its columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Majorness {
    /// Each row is a line (HLSL `row_major`, GLSL `row_major`).
    Row,
    /// Each column is a line (HLSL's default `column_major`, GLSL's default).
    Column,
}

/// The type of one constant, with the layout it has in its buffer written out: vectors are tightly packed lanes,
/// every matrix line and array element sits at a stated stride, whatever packing rules (D3D, std140, std430, DX9
/// registers) put it there.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstantType {
    Scalar(ScalarType),
    Vector(Value),
    /// `rows` by `columns` of `scalar`. The matrix is stored as lines (rows for [`Majorness::Row`], columns for
    /// [`Majorness::Column`]), `stride` bytes apart; each line's lanes are consecutive.
    Matrix {
        scalar: ScalarType,
        rows: u8,
        columns: u8,
        major: Majorness,
        stride: u32,
    },
    Array {
        element: Box<ConstantType>,
        count: u32,
        /// Bytes from the start of one element to the start of the next.
        stride: u32,
    },
    Struct {
        members: Vec<Member>,
    },
}

/// A structure's member: its offset is from the start of the structure.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Member {
    pub name: String,
    pub offset: u32,
    pub ty: ConstantType,
}

impl ConstantType {
    /// Bytes from the start of a value of this type to the end of its last byte of data: no padding after the last
    /// matrix line, array element or member (a buffer whose rules pad an array's tail states that in the
    /// buffer's size). A value with no data (a matrix of no lines or lanes, an array of no elements) is 0. `None` if the
    /// layout overflows 32 bits.
    pub fn size(&self) -> Option<u32> {
        match self {
            Self::Scalar(s) => Some(s.bytes()),
            Self::Vector(v) => u32::from(v.width).checked_mul(v.scalar.bytes()),
            Self::Matrix {
                scalar,
                rows,
                columns,
                major,
                stride,
            } => {
                let (lines, lanes) = match major {
                    Majorness::Row => (*rows, *columns),
                    Majorness::Column => (*columns, *rows),
                };
                if lines == 0 || lanes == 0 {
                    return Some(0);
                }
                u32::from(lines - 1)
                    .checked_mul(*stride)?
                    .checked_add(u32::from(lanes).checked_mul(scalar.bytes())?)
            }
            Self::Array {
                element,
                count,
                stride,
            } => match count.checked_sub(1) {
                None => Some(0),
                Some(before) => before.checked_mul(*stride)?.checked_add(element.size()?),
            },
            Self::Struct { members } => members
                .iter()
                .try_fold(0u32, |end, m| Some(end.max(m.offset.checked_add(m.ty.size()?)?))),
        }
    }

    /// The members of a structure, or of the structure an array holds (those of its first element): empty for any
    /// other type.
    pub fn members(&self) -> &[Member] {
        match self {
            Self::Struct { members } => members,
            Self::Array { element, .. } => element.members(),
            _ => &[],
        }
    }
}

/// One named constant of a buffer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Constant {
    pub name: String,
    /// Byte offset in the buffer.
    pub offset: u32,
    pub ty: ConstantType,
    /// The default value the source states, as bytes of the constant's layout.
    pub default: Option<Vec<u8>>,
}

/// A constant buffer's contents.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct BufferLayout {
    /// Bytes.
    pub size: u32,
    pub constants: Vec<Constant>,
}

/// What a resource is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// Constants by explicit byte offset. A program whose source states no names (a stripped one) has none here;
    /// a game's resolve pass names fields by offset.
    ConstantBuffer { layout: BufferLayout },
    Texture {
        dim: TextureDim,
        arrayed: bool,
        multisampled: bool,
        sample: SampleType,
    },
    Sampler {
        /// A comparison sampler (shadow lookups).
        comparison: bool,
    },
    /// A buffer the program reads.
    Buffer(BufferKind),
    /// An image or buffer the program may write.
    Storage { target: StorageTarget, access: Access },
    /// A ray tracing acceleration structure the program traces rays into.
    AccelerationStructure,
}

/// The register class of a D3D binding: `b`, `t`, `s` or `u`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum D3dClass {
    /// `b<n>`
    ConstantBuffer,
    /// `t<n>`
    ShaderResource,
    /// `s<n>`
    Sampler,
    /// `u<n>`
    UnorderedAccess,
}

/// A D3D bind point: class, register index and register space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct D3dBinding {
    pub class: D3dClass,
    pub register: u32,
    pub space: u32,
}

/// The source's own name and address for a resource; one field per API that has a notion of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ResourceProvenance {
    /// The name the source gives it (`Texture2D_8`, a constant buffer's name).
    pub name: String,
    pub d3d: Option<D3dBinding>,
    /// The resource this one is the hidden counter of: an unordered-access view's 32-bit counter (D3D's append and
    /// consume buffers, `IncrementCounter`), a buffer of its own in WebGPU. It has no D3D binding of its own; the
    /// renderer binds it with the view, starting from the view's initial count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counter_of: Option<ResourceId>,
    /// The members the source states for a view that reads constants: a D3D `tbuffer`'s variables (a constant block read
    /// through a shader-resource register, as a typed buffer). The resource's kind says how it is read; this is what it
    /// holds, by name and offset, as a constant buffer's layout is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<BufferLayout>,
}

/// One bound resource.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    /// How many consecutive bindings it spans: 1, or the length of a resource array. [`Resource::UNBOUNDED`] is an
    /// array whose length is the bound data's, not the program's (D3D's `-1`): nothing in the program says how far
    /// an index may go.
    pub count: u32,
    pub provenance: ResourceProvenance,
}

impl Resource {
    /// The `count` of an array the program does not size.
    pub const UNBOUNDED: u32 = u32::MAX;

    /// Whether the program sizes this array no further than the bound data does.
    pub fn is_unbounded(&self) -> bool {
        self.count == Self::UNBOUNDED
    }

    /// Whether it is an array of bindings, read by an index the program computes, rather than one binding.
    pub fn is_array(&self) -> bool {
        self.count != 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float(width: u8) -> ConstantType {
        ConstantType::Vector(Value {
            scalar: ScalarType::Float,
            width,
        })
    }

    /// Sizes worked out by hand: a line takes only its own lanes at the end, an array's last element only its own.
    #[test]
    fn sizes_end_at_the_last_byte_of_data() {
        assert_eq!(ConstantType::Scalar(ScalarType::Double).size(), Some(8));
        assert_eq!(float(3).size(), Some(12));
        let matrix = |rows, columns, major| ConstantType::Matrix {
            scalar: ScalarType::Float,
            rows,
            columns,
            major,
            stride: 16,
        };
        // 3 rows of 3 lanes: two full 16-byte lines and 12 bytes
        assert_eq!(matrix(3, 3, Majorness::Row).size(), Some(44));
        // 3 columns of 4 lanes: two full lines and 16 bytes
        assert_eq!(matrix(4, 3, Majorness::Column).size(), Some(48));
        let offsets = ConstantType::Array {
            element: Box::new(float(2)),
            count: 3,
            stride: 16,
        };
        assert_eq!(offsets.size(), Some(40));
        let empty = ConstantType::Array {
            element: Box::new(float(2)),
            count: 0,
            stride: 16,
        };
        assert_eq!(empty.size(), Some(0));
        // no lines or no lanes is no data, as an empty array: 0, not an overflow
        assert_eq!(matrix(0, 3, Majorness::Row).size(), Some(0));
        assert_eq!(matrix(3, 0, Majorness::Row).size(), Some(0));
        let record = ConstantType::Struct {
            members: vec![
                Member {
                    name: "a".into(),
                    offset: 0,
                    ty: float(3),
                },
                Member {
                    name: "b".into(),
                    offset: 12,
                    ty: ConstantType::Scalar(ScalarType::Float),
                },
                Member {
                    name: "c".into(),
                    offset: 16,
                    ty: offsets,
                },
            ],
        };
        assert_eq!(record.size(), Some(56));
        let overflow = ConstantType::Array {
            element: Box::new(float(4)),
            count: u32::MAX,
            stride: 16,
        };
        assert_eq!(overflow.size(), None);
    }

    #[test]
    fn members_are_found_through_arrays_of_structures() {
        let inner = ConstantType::Struct {
            members: vec![Member {
                name: "x".into(),
                offset: 0,
                ty: float(1),
            }],
        };
        let array = ConstantType::Array {
            element: Box::new(inner),
            count: 2,
            stride: 16,
        };
        assert_eq!(array.members().len(), 1);
        assert!(float(4).members().is_empty());
    }

    #[test]
    fn a_resource_serializes_by_kind_with_its_provenance() {
        let r = Resource {
            id: ResourceId(2),
            kind: ResourceKind::Texture {
                dim: TextureDim::D2,
                arrayed: false,
                multisampled: false,
                sample: SampleType::Float,
            },
            count: 1,
            provenance: ResourceProvenance {
                name: "Texture2D_8".into(),
                d3d: Some(D3dBinding {
                    class: D3dClass::ShaderResource,
                    register: 8,
                    space: 0,
                }),
                counter_of: None,
                layout: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            serde_json::json!({
                "id": 2,
                "kind": {"texture": {"dim": "d2", "arrayed": false, "multisampled": false, "sample": "float"}},
                "count": 1,
                "provenance": {
                    "name": "Texture2D_8",
                    "d3d": {"class": "shader_resource", "register": 8, "space": 0},
                },
            })
        );
    }
}
