//! `core.shader_binding`: on a material entity, which shader programs draw it and what each thing they read stands
//! for. It states roles, never frozen values: a bake or a renderer decides what a role is worth (a camera position,
//! the time, the frame's exposure) and says so.

use serde::Serialize;
use serde_json::Value;

use super::{BufferKind, Interface, Label, ResourceId, ResourceKind, Shader, Stage};
use crate::TextureLayers;
use crate::components::{Semantic, Texture, TextureFile};
use crate::json::JsonError;
use crate::pbr::Filters;
use crate::world::{Component, Entity};

/// What a pixel or vertex stage's input is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    /// A vertex attribute of the drawn mesh.
    Stream(Semantic),
    /// An input whose lanes are different vertex attributes the source packed into one register (two texture
    /// coordinate sets in one four-lane input): each entry is the attribute and the lanes it fills.
    Lanes(Vec<LaneStream>),
    /// The value the previous stage wrote under the same label.
    Previous,
    /// The pipeline supplies it (the varying's system value says which).
    System,
    /// Nothing known: the source's own meaning is not resolved, and why.
    Unknown { why: String },
}

/// The lanes `first..first + count` of an input: the first `count` components of the vertex attribute `semantic`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LaneStream {
    pub semantic: Semantic,
    /// The first lane (0 for x).
    pub first: u8,
    pub count: u8,
}

/// A camera or view-wide value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ViewParameter {
    CameraPosition,
    View,
    Projection,
    ViewProjection,
    /// The view-projection of a world with the camera at its origin: a program multiplies a world position minus the
    /// camera's ([`ViewParameter::CameraPosition`]) by it, so it is [`ViewParameter::ViewProjection`] without the camera's
    /// translation. The program takes the camera position from its own constant, so the two roles are bound together; a
    /// consumer that feeds the plain matrix here puts the geometry in the wrong place.
    CameraRelativeViewProjection,
    InverseView,
    InverseProjection,
    InverseViewProjection,
    /// The render target's size in pixels as two values: the width, then the height. A source that packs more into
    /// the register (the reciprocals beside the size, in either order) binds each pair by its own role over its own
    /// lanes.
    ViewportSize,
    /// The reciprocals of the render target's size in pixels as two values: `1 / width`, then `1 / height`, the
    /// uv-space step of a whole texel.
    InverseViewportSize,
    /// The distance from the camera to the near clip plane, along the view axis, in world units (one value).
    NearPlane,
    /// The distance from the camera to the far clip plane, along the view axis, in world units (one value).
    FarPlane,
    /// `1 / far plane`: the reciprocal of [`ViewParameter::FarPlane`] (one value).
    InverseFarPlane,
    /// The sign the projection puts on up (one value): +1 when a point above the view axis, in view space, has a positive
    /// clip-space y, -1 when the projection negates y (a render into a texture, on a source whose texture origin is not
    /// the screen's). A source that stores the opposite convention maps its value onto this one.
    ProjectionFlip,
    /// The previous frame's view-projection matrix (what a motion vector is measured against).
    PreviousViewProjection,
}

/// A transform built from the drawn object's object-to-world matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectMatrix {
    /// Object space to world space.
    World,
    /// Object space to view space.
    WorldView,
    /// Object space to clip space.
    WorldViewProjection,
}

/// A value of the drawn object or instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ObjectParameter {
    /// The object-to-world matrix.
    World,
    InverseWorld,
    WorldViewProjection,
    /// The object-to-view matrix.
    WorldView,
    /// The transform as the previous frame had it: the object's previous pose, the previous view and projection (what a
    /// motion vector is measured from).
    PreviousFrame(ObjectMatrix),
    /// The transform in the engine's camera-relative form (the world with the camera at its origin): a program computes
    /// its clip position, and its previous-frame position, from this or from the plain transform, the one the draw
    /// supplies (by the engine's own name for it; no capture checks the translation).
    CameraRelative(ObjectMatrix),
    /// The previous frame's transform, with the previous frame's camera at the origin of world space.
    CameraRelativePreviousFrame(ObjectMatrix),
    /// A colour the instance tints the surface with.
    InstanceColour,
    /// The scale (`xy`) and offset (`zw`) a model's texture coordinate set `n` is drawn with: the engine feeds one
    /// value per model for each set, a scroll that animates some models (water, flags).
    TextureScroll(u8),
    /// The matrices of a skinned draw's joints, each three rows of four (the object-to-world transform of the joint
    /// the pose gives, in the engine's own palette order): what a skinning program blends by the vertex's joint indices
    /// and weights.
    BonePalette,
    /// The instance's environment data word (an unsigned integer the engine writes per instance; the layered programs
    /// scale it by 1/255 into the alpha lane of a G-buffer target).
    EnvironmentData,
    /// The index of the instance's first matrix in the object-matrix palette, which the vertex program adds to the
    /// index it takes (the joint indices of a skinned vertex, the instance's own for a rigid one).
    MatrixBase,
    /// The instance's user clip plane (the vertex program's clip distance against it).
    ClipPlane,
    /// Row `n` (0 or 1) of an animated affine transform of the first texture coordinate set, three values each:
    /// `u' = dot(row 0, (u, v, 1))`, `v' = dot(row 1, (u, v, 1))`. Some models animate their textures this way.
    TextureTransformRow(u8),
    /// Where along the rows of a palette texture the instance's tint selects (x): the v the program samples the
    /// palette at, the u being a vertex colour.
    PaletteCoordinate,
    /// Whether the draw fades by the camera's distance, dithered (a flag the engine sets per draw): a program that reads it
    /// keeps a fragment where `distance * CameraFadeInverseRange - CameraFadeEnd` is not below the dither noise.
    CameraFadeEnabled,
    /// The camera fade's end term, subtracted from the distance scaled by [`ObjectParameter::CameraFadeInverseRange`].
    CameraFadeEnd,
    /// The camera fade's inverse range: the factor the fragment's distance from the camera is scaled by.
    CameraFadeInverseRange,
    /// Whether the instance is drawn with dithered transparency (a per-draw flag): a program that reads it, with
    /// [`ObjectParameter::DitherEnabled`] set, discards a fragment by the dither noise against the amount the instance record
    /// carries.
    DitherAlphaEnabled,
    /// The second per-model flag of the dithered transparency, set together with [`ObjectParameter::DitherAlphaEnabled`]
    /// (a program reads both, and fades only where both are on).
    DitherEnabled,
}

/// A value of the frame being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FrameParameter {
    /// Seconds since a start the game defines.
    Time,
    TimeOfDay,
    /// The exposure the frame's output is scaled by.
    Exposure,
    /// An index into a palette of colours a draw selects from.
    ColourSlot,
    /// The direction the dominant directional light travels (xyz): a surface is lit by `saturate(dot(normal, -xyz))`.
    LightDirection,
    /// The colour (rgb) of that light.
    LightColour,
    /// The directional ambient light as four values: its colour (xyz: a lighting program adds each channel to the same
    /// channel of the lit colour, scaled by how much the surface faces an ambient direction it also reads), and a w that
    /// scales the ambient terms a vertex carries, growing from 1 (no change) with distance.
    DirectionalAmbientColour,
}

/// What a constant of a buffer stands for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstantRole {
    /// The material's own parameter, as stored: the 32-bit words the constant takes (bit patterns, so an integer
    /// parameter and a float one are both exact).
    Material(Vec<u32>),
    View(ViewParameter),
    Object(ObjectParameter),
    Frame(FrameParameter),
    /// Not resolved to a role, and why.
    Unknown {
        why: String,
    },
}

/// One constant: the bytes `offset..offset + size` of a constant buffer resource of the shader, named when the
/// source names it (a stripped program has fields by offset only), and what it stands for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConstantBinding {
    pub resource: ResourceId,
    /// Byte offset in the buffer.
    pub offset: u32,
    /// Bytes.
    pub size: u32,
    pub name: Option<String>,
    pub role: ConstantRole,
}

/// What a texture coordinate outside 0 to 1 reads, along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressMode {
    /// The texture repeats.
    Repeat,
    /// The texture repeats, every other copy mirrored.
    Mirror,
    /// The edge texel.
    Clamp,
    /// The sampler's border colour.
    Border,
    /// Mirrored once about the origin, then the edge texel.
    MirrorOnce,
}

/// A sampler's state as the source stores it, for a texture slot: each part `None` where the source states none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct SamplerState {
    /// Addressing along u, v and w.
    pub wrap: [Option<AddressMode>; 3],
    pub filters: Filters,
    /// The most texels along the axis of anisotropy an anisotropic filter takes (1: none beyond the filters'), where
    /// the source states an anisotropic filter.
    pub max_anisotropy: Option<u8>,
    /// The comparison a depth-comparison sampler makes of the reference against the texel, where the sampler is one.
    pub compare: Option<Compare>,
    /// Added to the mip level the hardware computes, where the source states a non-zero bias.
    pub mip_lod_bias: Option<f32>,
}

/// An image of the frame being drawn that a program reads: the consumer supplies it from its own frame, as it supplies a view or
/// frame constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FrameTexture {
    /// The depth the frame's depth pass wrote, the hardware depth in `x`: a program that reads it linearises it with the scene
    /// constants' depth parameters (soft edges of fog, foam and water against the geometry behind).
    SceneDepth,
    /// The colour image of what the frame has drawn so far behind the surface (the opaque scene, or the screen as it
    /// stands): what a refracting or see-through surface samples.
    SceneColour,
    /// The depth image rendered from a shadow-casting light (the main light's, its cascades side by side where it has
    /// them): a program compares a surface's depth in that light's space with it.
    ShadowMap,
    /// The shadow term of the main light gathered per screen pixel: a program samples it at its own screen position, 1
    /// where the light reaches and 0 where it is shadowed.
    ScreenSpaceShadows,
    /// The ambient occlusion the frame computed per screen pixel (1 unoccluded).
    AmbientOcclusion,
    /// The cube image of the environment around the surface as the nearest reflection probe, or the sky where there is
    /// none, captured: what a reflection samples along the reflected view direction.
    ReflectionProbe,
    /// The volume image of the light probes' lighting around the surface (a 3D image of spherical-harmonic
    /// coefficients the surface samples at its position).
    LightProbeVolume,
}

impl FrameTexture {
    /// The dimension of the image: a probe's is a cube, the light probes' a volume, every other a plane.
    pub fn dim(self) -> super::TextureDim {
        match self {
            Self::ReflectionProbe => super::TextureDim::Cube,
            Self::LightProbeVolume => super::TextureDim::D3,
            Self::SceneDepth
            | Self::SceneColour
            | Self::ShadowMap
            | Self::ScreenSpaceShadows
            | Self::AmbientOcclusion => super::TextureDim::D2,
        }
    }
}

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
}

/// One texture slot: the texture resource of the shader, the sampler resource it is read through when the program
/// has one, and what is bound.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextureBinding {
    pub resource: ResourceId,
    pub sampler: Option<ResourceId>,
    pub source: TextureSource,
}

/// One buffer slot: a buffer resource of the shader (a structured, raw or typed buffer a program indexes at run time) and
/// what the source binds to it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BufferBinding {
    pub resource: ResourceId,
    pub source: BufferSource,
}

/// What a buffer slot holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferSource {
    /// A table of the draw's own data, by what it holds: the consumer fills it from the draw it makes.
    Table(BufferTable),
    /// Nothing is known of the contents and the source says why: a consumer states the stand-in it uses, and never
    /// reads the buffer as zeros without saying so (as [`TextureSource::Missing`] for a texture, or an unknown
    /// constant).
    Unknown { why: String },
}

/// A table of the draw's data a program reads from a buffer.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BufferTable {
    /// The matrices of the draw: the same content as [`ObjectParameter::BonePalette`] (the object-to-world transforms,
    /// in the engine's own order, each `rows` rows of four floats: three for an affine transform without its last row,
    /// four for a full matrix) in a buffer instead of constants, and with that role's row convention. They lie one
    /// matrix after another, so element `i` of the buffer is row `i % rows` of matrix `i / rows`; the elements are
    /// four floats, 16 bytes.
    ObjectMatrices { rows: u8 },
    /// One record per instance drawn, `stride` 32-bit words each, in instance order: word `matrix_start` of a record is
    /// the index, in elements of the table `matrices` (a [`BufferTable::ObjectMatrices`] of the same stage), of the
    /// instance's first row. The program adds the indices of the matrices it takes (a joint's, a model's own) to it.
    Instances {
        stride: u8,
        matrix_start: u8,
        matrices: ResourceId,
    },
}

/// Where a normal vector is expressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalSpace {
    World,
    View,
    Tangent,
}

/// How a normal is stored in the lanes of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalEncoding {
    /// x, y, z as signed values.
    Xyz,
    /// x, y, z mapped to 0-1 (`n * 0.5 + 0.5`).
    XyzUnorm,
    /// x and y signed, z from the unit length.
    Xy,
    /// x and y mapped to 0-1, z from the unit length.
    XyUnorm,
    /// x, y, z stored as `n * 0.5 + bias` (the bias given by its f32 bits): decoded `(v - bias) * 2`.
    XyzBiased { bias_bits: u32 },
    /// Octahedral mapping in two lanes, 0-1.
    Octahedral,
    /// Octahedral in the diamond layout over three 8-bit UNORM lanes: lanes 0 and 1 are the low eight bits of two 9-bit
    /// coordinates `u` and `v` (`round(255 (c + 1))`, 0 to 510 over -1 to 1), lane 2's bit 1 and bit 2 their ninth bits and
    /// its bit 0 the sign of z; the lane's other five bits are not part of the normal (a payload byte, which an engine
    /// may keep a class in). `x = (u + v) / 2`, `y = (u - v) / 2`, `|z| = 1 - |x| - |y|`, normalised
    /// ([`NormalEncoding::decode_octahedral_diamond`]).
    OctahedralDiamond,
}

impl NormalEncoding {
    /// The normal and the payload byte of three UNORM lanes stored as [`NormalEncoding::OctahedralDiamond`]: each lane
    /// is taken as the 8-bit value it rounds to.
    pub fn decode_octahedral_diamond(lanes: [f32; 3]) -> ([f64; 3], u8) {
        let [r, g, b] = lanes.map(|x| (x.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8);
        let u9 = f64::from(b >> 1 & 1) * 256.0 + f64::from(r);
        let v9 = f64::from(b >> 2 & 1) * 256.0 + f64::from(g);
        let pu = (u9 / 255.0 - 1.0).clamp(-1.0, 1.0);
        let pv = (v9 / 255.0 - 1.0).clamp(-1.0, 1.0);
        let (x, y) = ((pu + pv) / 2.0, (pu - pv) / 2.0);
        let z = (1.0 - x.abs() - y.abs()) * if b & 1 == 1 { 1.0 } else { -1.0 };
        let n = (x * x + y * y + z * z).sqrt();
        ([x / n, y / n, z / n], b >> 3)
    }

    /// The three 8-bit lanes [`NormalEncoding::OctahedralDiamond`] stores a normal and a payload byte (below 32) as;
    /// a z of 0 stores as positive. A vector that is not zero is normalised by the encoding itself.
    pub fn encode_octahedral_diamond(normal: [f64; 3], payload: u8) -> [u8; 3] {
        let [x, y, z] = normal;
        let sum = x.abs() + y.abs() + z.abs();
        let (x, y) = (x / sum, y / sum);
        let nine = |c: f64| ((c * 255.0 + 255.0).round().clamp(0.0, 510.0)) as u16;
        let (u9, v9) = (nine(x + y), nine(x - y));
        let sign = u8::from(z >= 0.0);
        [
            (u9 & 0xff) as u8,
            (v9 & 0xff) as u8,
            sign | ((u9 >> 8) as u8) << 1 | ((v9 >> 8) as u8) << 2 | (payload & 0x1f) << 3,
        ]
    }
}

/// How a scalar is stored in the lanes of an output: `stored = (value / scale)^(1 / power)`, so the value is
/// `stored^power * scale` (the scale given by its f32 bits). Power 1 is linear, 2 a square root (a value kept in a
/// perceptual range), 4 a fourth root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScalarEncoding {
    /// The power the stored value is raised to, to give the value over the scale: 1 for none, 2 for the square root
    /// stored, 4 for the fourth root. Never 0.
    pub power: u8,
    /// The scale's f32 bits (1.0 for none): bits so the encoding compares and hashes exactly; it is written as the
    /// number `scale`.
    pub scale_bits: u32,
}

impl Serialize for ScalarEncoding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("ScalarEncoding", 2)?;
        out.serialize_field("power", &self.power)?;
        out.serialize_field("scale", &f32::from_bits(self.scale_bits))?;
        out.end()
    }
}

impl ScalarEncoding {
    /// The value as it is (scale 1).
    pub const LINEAR: Self = Self {
        power: 1,
        scale_bits: 0x3F80_0000,
    };

    /// Stored as the value over `scale`, no power.
    pub fn linear_over(scale: f32) -> Self {
        Self::power_of(1, scale)
    }

    /// Stored as the square root of the value over `scale`.
    pub fn square_root_of(scale: f32) -> Self {
        Self::power_of(2, scale)
    }

    /// Stored as the `power`th root of the value over `scale` (`power` 0 is taken as 1: no power).
    pub fn power_of(power: u8, scale: f32) -> Self {
        Self {
            power: power.max(1),
            scale_bits: scale.to_bits(),
        }
    }

    /// The value for a stored one.
    pub fn decode(&self, stored: f32) -> f32 {
        stored.powi(i32::from(self.power)) * f32::from_bits(self.scale_bits)
    }
}

/// What an [`OutputRole::Occlusion`] occludes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Occluded {
    /// Every light that reaches the surface from outside it: the one occlusion a source has.
    AllLight,
    /// The ambient light of the environment itself (the sky and what it scatters): the term a lighting pass scales that
    /// ambient by, where the source keeps another occlusion for the light of placed artificial sources.
    NaturalAmbient,
    /// The ambient light of placed artificial sources (interior and exterior ambient volumes), apart from the
    /// environment's own ([`Occluded::NaturalAmbient`]).
    ArtificialAmbient,
}

/// What lanes of an output stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutputRole {
    Albedo,
    Normal {
        encoding: NormalEncoding,
        space: NormalSpace,
    },
    /// Smoothness, 1 - roughness.
    Gloss,
    Roughness,
    Metallic,
    /// The factor the light `of` is scaled by where the surface is shadowed from it (1 unoccluded, 0 fully): the value
    /// decoded by `encoding`, as the lighting pass multiplies the light by it.
    Occlusion {
        of: Occluded,
        encoding: ScalarEncoding,
    },
    /// How much of the light a surface reflects as a highlight is blocked where the surface faces away from the
    /// environment (1 unblocked): the term a lighting pass multiplies its specular by, apart from the diffuse
    /// [`OutputRole::Occlusion`].
    SpecularOcclusion,
    /// How much light passes through the surface (a leaf, a petal): a lighting pass tints the albedo by it and adds it
    /// to the lit colour, 0 for an opaque surface.
    Translucency,
    /// Specular reflectance at normal incidence.
    Reflectance,
    /// The strength of the specular term, how much light the surface reflects as a highlight (0 to 1 once decoded).
    SpecularIntensity {
        encoding: ScalarEncoding,
    },
    /// How tight the specular highlight is: the material's falloff or exponent as the source's lighting uses it
    /// (what lighting derives from it, a Blinn-Phong exponent for instance, is the source's).
    SpecularExponent {
        encoding: ScalarEncoding,
    },
    /// The source's Fresnel term for the surface, as the source stores it (how the lighting turns it into a
    /// reflectance at grazing angles is the source's).
    Fresnel,
    /// The screen-space displacement, in pixels, from the fragment's position in the frame being drawn to its
    /// position in the previous frame (x right, y down).
    MotionVector,
    /// The light the surface gives off: a scalar in one lane that scales the albedo (the lighting pass adds the albedo
    /// times it to the lit colour) or a colour in three, the value decoded by `encoding`.
    Emission {
        encoding: ScalarEncoding,
    },
    /// The finished colour of a forward pass.
    Colour,
    /// Something the source writes that is not resolved to a role: the output's index.
    Unknown(u32),
}

/// Lanes of one output varying and their role: an output's lanes may carry different things (albedo in rgb and
/// occlusion in a), as several bindings of one label.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputBinding {
    pub label: Label,
    /// Lane mask: bit k is lane k of the output's value.
    pub lanes: u8,
    pub role: OutputRole,
}

/// A stage's input binding: the role of an input varying the shader declares.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputBinding {
    pub label: Label,
    pub role: InputRole,
}

/// What picks one of several stages of one kind in a binding: the mesh-side field a consumer reads, named by the dialect
/// component and field that holds it (`anvil.layout.kind`), and the value that picks this stage. A consumer draws a mesh
/// with the stage whose key names a field it has and whose value that field equals; a mesh whose value matches no stage
/// is refused by name, not drawn with a first or default stage.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct VariantKey {
    /// The component and field the mesh carries the key in, as `component.field` (`anvil.layout.kind`), or the name
    /// the source gives its keys where no mesh field holds them (`frostbite.variant`).
    pub field: String,
    pub value: u32,
    /// What the key stands for, in the source's terms. A description only: it never identifies a variant (a binding's
    /// check, and a consumer picking a stage, go by `field` and `value`) and may be empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<KeyPart>,
}

/// One named number of what a [`VariantKey`] stands for (a program's place in its row, a pixel output count, a
/// permutation id). It describes and never identifies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct KeyPart {
    pub name: String,
    pub value: u64,
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
    /// The buffers the program indexes at run time (tables of the draw's matrices and instances), for a source that
    /// states them; empty for a source that does not, and a buffer no entry names is one nothing is known of.
    pub buffers: Vec<BufferBinding>,
    pub outputs: Vec<OutputBinding>,
}

/// A blend factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendFactor {
    Zero,
    One,
    SrcColour,
    OneMinusSrcColour,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstColour,
    OneMinusDstColour,
    DstAlpha,
    OneMinusDstAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

/// `op(src * source, dst * destination)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BlendEquation {
    pub source: BlendFactor,
    pub destination: BlendFactor,
    pub op: BlendOp,
}

/// Blending of the colour and the alpha channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Blend {
    pub colour: BlendEquation,
    pub alpha: BlendEquation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cull {
    None,
    Front,
    Back,
}

/// Depth test and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct DepthState {
    pub test: Compare,
    pub write: bool,
}

/// Alpha test: a pixel is kept where `alpha compare reference`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AlphaTest {
    pub compare: Compare,
    pub reference: f32,
}

/// A stencil operation (D3D12 and Vulkan have the same eight).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StencilOp {
    Keep,
    Zero,
    Replace,
    /// Increment, clamped at the maximum.
    IncrementClamp,
    /// Decrement, clamped at zero.
    DecrementClamp,
    Invert,
    /// Increment, wrapping to zero.
    IncrementWrap,
    /// Decrement, wrapping to the maximum.
    DecrementWrap,
}

/// What the stencil test does for one facing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct StencilFace {
    /// On a failed stencil test.
    pub fail: StencilOp,
    /// On a passed stencil test and a failed depth test.
    pub depth_fail: StencilOp,
    /// On both passed.
    pub pass: StencilOp,
    pub compare: Compare,
}

/// An enabled stencil test (a disabled one is `None` in [`RenderState::stencil`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Stencil {
    pub read_mask: u8,
    pub write_mask: u8,
    /// The reference value the test compares against and `Replace` writes, where the source states one (D3D12 sets it
    /// per draw: a source that stores it per draw states none).
    pub reference: Option<u8>,
    pub front: StencilFace,
    pub back: StencilFace,
}

/// What one render target does with a fragment: its blend and which channels it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TargetState {
    /// `None` where the target is not blended.
    pub blend: Option<Blend>,
    /// The channels written: bit 0 red, 1 green, 2 blue, 3 alpha (`0xf` all).
    pub write_mask: u8,
}

/// A depth bias, D3D12's three terms: `bias = constant * r + slope_scaled * max_slope`, limited to `clamp` when that
/// is not zero (r the smallest resolvable depth difference).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DepthBias {
    pub constant: f32,
    pub slope_scaled: f32,
    pub clamp: f32,
}

/// How a triangle is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    Solid,
    Wireframe,
}

/// Fixed-function state as the source stores it; each `None` where it states none. D3D12-shaped: `blend` is render
/// target 0's blend (what a consumer with one render target reads), and `targets` has every render target's blend
/// and write mask where the source states them, with `targets[0]`'s blend equal to `blend` ([`ShaderBinding`]'s
/// verification checks it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct RenderState {
    pub blend: Option<Blend>,
    pub depth: Option<DepthState>,
    pub cull: Option<Cull>,
    pub alpha_test: Option<AlphaTest>,
    /// Per render target, in order.
    pub targets: [Option<TargetState>; 8],
    /// The stencil test, where one is enabled.
    pub stencil: Option<Stencil>,
    pub depth_bias: Option<DepthBias>,
    pub fill: Option<Fill>,
    /// Whether counter-clockwise triangles are front faces.
    pub front_counter_clockwise: Option<bool>,
}

impl RenderState {
    /// The state is consistent: the write masks fit four channels, the bias is finite, and `blend` is the first
    /// target's blend (a state with a blend has a first target, and the other way round).
    pub fn verify(&self) -> Result<(), String> {
        if let Some(a) = self.alpha_test
            && !a.reference.is_finite()
        {
            return Err("the alpha test's reference is not finite".into());
        }
        for (i, t) in self.targets.iter().enumerate() {
            if let Some(t) = t
                && t.write_mask > 0xf
            {
                return Err(format!(
                    "render target {i}'s write mask {:#x} is wider than four channels",
                    t.write_mask
                ));
            }
        }
        match self.targets[0] {
            Some(first) if first.blend != self.blend => {
                return Err("render target 0's blend is not the state's blend".into());
            }
            None if self.blend.is_some() => {
                return Err("the state has a blend and render target 0 has no state to hold it".into());
            }
            _ => {}
        }
        if let Some(b) = self.depth_bias
            && !(b.constant.is_finite() && b.slope_scaled.is_finite() && b.clamp.is_finite())
        {
            return Err("the depth bias is not finite".into());
        }
        Ok(())
    }
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

    fn verify(&self, w: &crate::World) -> Result<(), String> {
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
        self.verify_buffers(interface).map_err(at)
    }

    /// The buffer slots: each a buffer of the shader, bound once, a matrix table of 16-byte elements, and an instance
    /// table whose records fit its stride and name a matrix table of this stage.
    fn verify_buffers(&self, interface: &Interface) -> Result<(), String> {
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
                BufferSource::Table(BufferTable::ObjectMatrices { rows }) => {
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
                                BufferSource::Table(BufferTable::ObjectMatrices { .. })
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
            s.verify(w)?;
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

    /// The diamond octahedral store round-trips: the hand-worked axes (+z stores x = y = 255 and the sign bit; +x
    /// stores u = v = 510, both ninth bits; -y stores u = 0, v = 510), the payload kept, and a grid of normals to
    /// the 9-bit step.
    #[test]
    fn the_diamond_octahedral_decode_inverts_its_encoding() {
        let unorm = |lanes: [u8; 3]| lanes.map(|v| f32::from(v) / 255.0);
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9);
        let (n, class) = NormalEncoding::decode_octahedral_diamond(unorm([255, 255, 5 << 3 | 1]));
        assert!(near(n, [0.0, 0.0, 1.0]) && class == 5, "{n:?}");
        let (n, _) = NormalEncoding::decode_octahedral_diamond(unorm([254, 254, 5 << 3 | 0b111]));
        assert!(near(n, [1.0, 0.0, 0.0]), "{n:?}");
        let (n, _) = NormalEncoding::decode_octahedral_diamond(unorm([0, 254, 5 << 3 | 0b101]));
        assert!(near(n, [0.0, -1.0, 0.0]), "{n:?}");
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([0.0, 0.0, 1.0], 5),
            [255, 255, 5 << 3 | 1]
        );
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([1.0, 0.0, 0.0], 5),
            [254, 254, 5 << 3 | 0b111]
        );
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([0.0, -1.0, 0.0], 5),
            [0, 254, 5 << 3 | 0b101]
        );
        // a grid over the sphere, both hemispheres, every payload class that matters
        let mut worst = 1.0f64;
        for i in 0..40 {
            for j in 0..80 {
                let (theta, phi) = (
                    std::f64::consts::PI * (f64::from(i) + 0.5) / 40.0,
                    std::f64::consts::TAU * f64::from(j) / 80.0,
                );
                let n = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()];
                let class = (i % 6) as u8;
                let lanes = NormalEncoding::encode_octahedral_diamond(n, class);
                let (back, payload) = NormalEncoding::decode_octahedral_diamond(unorm(lanes));
                assert_eq!(payload, class);
                worst = worst.min(n[0] * back[0] + n[1] * back[1] + n[2] * back[2]);
            }
        }
        // a 9-bit coordinate step is 2/510: the direction is within a few of them
        assert!(worst > 1.0 - 2e-4, "worst dot {worst}");
    }
    use crate::World;
    use crate::shader::{
        BufferLayout, Constant, ConstantType, ResourceProvenance, SampleType, ScalarType, ShaderFormat,
        TextureDim, Value as Lanes, Varying, VaryingProvenance,
    };
    use crate::shader::{Resource, ResourceKind as Kind};

    #[test]
    fn a_scalar_encoding_decodes_what_it_stores() {
        // stored = sqrt(value / 512): the value is the stored one squared times 512
        let e = ScalarEncoding::square_root_of(512.0);
        assert_eq!(e.decode(0.5), 128.0);
        assert_eq!(ScalarEncoding::LINEAR.decode(0.25), 0.25);
        assert_eq!(ScalarEncoding::square_root_of(1.0).decode(0.5), 0.25);
        // a fourth root over 4: the value is the stored one to the fourth, times 4
        let fourth = ScalarEncoding::power_of(4, 4.0);
        assert_eq!(fourth.decode(0.5), 0.25);
        assert_eq!(ScalarEncoding::linear_over(16.0).decode(0.25), 4.0);
        // written as the number it is, not its bit pattern
        assert_eq!(
            serde_json::to_value(fourth).unwrap(),
            serde_json::json!({"power": 4, "scale": 4.0})
        );
        assert_eq!(
            serde_json::to_value(ScalarEncoding::linear_over(0.0625)).unwrap(),
            serde_json::json!({"power": 1, "scale": 0.0625})
        );
        // a power of 0 is no power
        assert_eq!(ScalarEncoding::power_of(0, 2.0), ScalarEncoding::linear_over(2.0));
    }

    /// The camera-relative view-projection is a role of its own: a source that states it is not read as the plain matrix.
    #[test]
    fn a_camera_relative_view_projection_is_not_the_plain_one() {
        assert_ne!(
            ConstantRole::View(ViewParameter::CameraRelativeViewProjection),
            ConstantRole::View(ViewParameter::ViewProjection)
        );
        assert_eq!(
            serde_json::to_value(ViewParameter::CameraRelativeViewProjection).unwrap(),
            serde_json::json!("camera_relative_view_projection")
        );
    }

    #[test]
    fn a_render_state_is_consistent() {
        let eq = BlendEquation {
            source: BlendFactor::SrcAlpha,
            destination: BlendFactor::OneMinusSrcAlpha,
            op: BlendOp::Add,
        };
        let blend = Blend {
            colour: eq,
            alpha: eq,
        };
        let mut state = RenderState::default();
        assert_eq!(state.verify(), Ok(()));
        // the first target's blend is the state's
        state.targets[0] = Some(TargetState {
            blend: Some(blend),
            write_mask: 0xf,
        });
        assert!(state.verify().unwrap_err().contains("target 0's blend"));
        state.blend = Some(blend);
        assert_eq!(state.verify(), Ok(()));
        // a blend with no state for the first target to hold it is not the same as no blend
        let mut lone = RenderState {
            blend: Some(blend),
            ..RenderState::default()
        };
        assert!(lone.verify().unwrap_err().contains("no state to hold it"));
        lone.targets[0] = Some(TargetState {
            blend: Some(blend),
            write_mask: 0xf,
        });
        assert_eq!(lone.verify(), Ok(()));
        // other targets are free; a mask is four channels
        state.targets[3] = Some(TargetState {
            blend: None,
            write_mask: 0x1f,
        });
        assert!(state.verify().unwrap_err().contains("target 3's write mask"));
        state.targets[3] = Some(TargetState {
            blend: None,
            write_mask: 0x8,
        });
        state.depth_bias = Some(DepthBias {
            constant: 0.0,
            slope_scaled: f32::NAN,
            clamp: 0.0,
        });
        assert!(state.verify().unwrap_err().contains("depth bias"));
    }

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
                    role: InputRole::Stream(Semantic::TexCoord(0)),
                }],
                constants: vec![
                    ConstantBinding {
                        resource: ResourceId(0),
                        offset: 0,
                        size: 16,
                        name: Some("tint".into()),
                        role: ConstantRole::Material(vec![1.0f32.to_bits(); 4]),
                    },
                    ConstantBinding {
                        resource: ResourceId(0),
                        offset: 16,
                        size: 16,
                        name: None,
                        role: ConstantRole::Unknown {
                            why: "cb0[1] is not in the recipe".into(),
                        },
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
        let structured =
            |id, stride| resource(id, Kind::Buffer(crate::shader::BufferKind::Structured { stride }));
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

    /// The buffer slots: a matrix table and the instance records that index it verify, each rule broken once is named,
    /// and the roles serialize under their own names.
    #[test]
    fn buffer_slots_name_the_tables_the_program_indexes() {
        let mut w = World::new();
        let (s, m) = (w.spawn(), w.spawn());
        w.insert(s, buffer_shader());
        let good = vec![matrices(10, 3), instances(11, 2, 0, 10)];
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
            buffer_binding(s, vec![matrices(10, 4), unknown("the engine fills it")]),
        );
        assert_eq!(w.get::<ShaderBinding>(m).unwrap().verify(m, &w), Ok(()));
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
