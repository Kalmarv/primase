//! What each thing a shader program reads and writes stands for: input, constant, texture, buffer and output roles,
//! their encodings, and render state. It states roles, never frozen values: a bake or a renderer decides what a role is worth (a camera position,
//! the time, the frame's exposure) and says so.

use serde::Serialize;

use crate::{Label, ResourceId};

/// A vertex attribute a program's input reads: what the drawn mesh's stream holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Attribute {
    Position,
    Normal,
    /// Tangent set: xyz tangent, w the bitangent sign. Set 0 is glTF's standard TANGENT.
    Tangent(u8),
    /// u right, v down: (0, 0) is the image's top-left corner (glTF).
    TexCoord(u8),
    Color(u8),
    Joints(u8),
    Weights(u8),
}

/// What a pixel or vertex stage's input is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    /// A vertex attribute of the drawn mesh.
    Stream(Attribute),
    /// An input whose lanes are different vertex attributes the source packed into one register (two texture
    /// coordinate sets in one four-lane input), or one component of an attribute the source feeds as an input of its own
    /// (a tangent's sign as a byte): each entry is the attribute, the components it takes and the lanes it fills.
    Lanes(Vec<LaneStream>),
    /// The value the previous stage wrote under the same label.
    Previous,
    /// The pipeline supplies it (the varying's system value says which).
    System,
    /// Nothing known: the source's own meaning is not resolved, and why.
    Unknown { why: String },
}
/// The lanes `first..first + count` of an input: `count` components of the vertex attribute `semantic`, from component
/// `component` (the attribute's own components: x 0, y 1, z 2, w 3), each in the encoding the input's scalar type has, as
/// the other packed inputs take theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LaneStream {
    pub semantic: Attribute,
    /// The first lane (0 for x).
    pub first: u8,
    pub count: u8,
    /// The first component of the attribute lane `first` takes (0 for all but the programs that take one component of an
    /// attribute apart: a source game's cloth programs read the tangent's w, the bitangent sign, as the one
    /// byte of an input named BINORMAL0, as `(byte - 127) / 127` where the other permutations read TANGENT0.w; inferred
    /// from the program's decode of it, not captured from the game).
    pub component: u8,
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
    /// The matrices of a skinned draw's joints, each a 3 x 4 transform (the object-to-world transform of the joint
    /// the pose gives, in the engine's own palette order), its lines as the binding states ([`MatrixLines`]): what a
    /// skinning program blends by the vertex's joint indices and weights.
    BonePalette,
    /// The matrices of a second, short palette a skinned program takes the position through before
    /// [`ObjectParameter::BonePalette`] (the engine's name for it is the cloth-cancel matrices): each a 3 x 4
    /// transform held as the binding states ([`MatrixLines`]), chosen by the same joint index plus the instance's
    /// matrix base ([`ObjectParameter::MatrixBase`]).
    /// What it holds when the draw simulates no cloth is not established; a consumer that feeds the identity makes
    /// the first step leave the position as it is.
    ClothCancelPalette,
    /// The instance's environment data word (an unsigned integer the engine writes per instance; the layered programs
    /// scale it by 1/255 into the alpha lane of a G-buffer target).
    EnvironmentData,
    /// The index of the instance's first matrix in the object-matrix palette, which the vertex program adds to the
    /// index it takes (the joint indices of a skinned vertex, the instance's own for a rigid one).
    MatrixBase,
    /// The index of the draw's first record in the instance table ([`BufferTable::Instances`]), an unsigned integer word:
    /// the vertex program adds the instance number to it and reads that record. It counts records of the instance table,
    /// where [`ObjectParameter::MatrixBase`] counts matrices of the palette. A draw of one instance that starts the table
    /// has 0.
    InstanceBase,
    /// Whether the draw simulates cloth: a nonzero word where the vertex program takes the cloth's matrix instead of the
    /// palette's. A consumer that draws a static or rest pose states 0, which runs the palette arm.
    ClothSimulation,
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
    /// A reference confirmed by the program's discard graph. `compared` states whether a portable cutoff on output
    /// alpha is the same test or an approximation. A bake freezes the reference below the alpha's range, and must
    /// also neutralize any stated dither perturbation; the draw uses the material's original `value`.
    AlphaReference {
        value: f32,
        compared: AlphaComparison,
    },
    /// Not resolved to a role, and why.
    Unknown {
        why: String,
    },
}
/// The scalar compared against an alpha reference, proved from the program's data flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaComparison {
    /// The output alpha itself (including unchanged branch arms).
    Output,
    /// A scalar that reaches output alpha through arithmetic. Cutting the output is an approximation.
    Transformed,
    /// A pre-transform alpha plus `(dither.a - 0.5) * scale`. `scale_word` is a word offset in the reference's own
    /// buffer. A hard cutoff on output alpha approximates the game's stochastic, pre-transform test.
    Dithered { scale_word: u32 },
    /// The discard compares a value made of texture samples and constants that reaches no target's alpha (a G-buffer
    /// program writes none: `t1.w * t0.w + scale * (noise - 0.5) < reference`). A bake freezes the reference
    /// below the range as for the others. The test is the program's own expression, read exactly from its graph
    /// (`iridia::output_uses::alpha_tests`); a cutoff on any output alpha is not it, and a consumer that cuts
    /// evaluates that expression with the values its binding states, or does not cut.
    Unwritten,
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
    /// For a matrix role ([`ConstantRole::is_matrix`]), how its 16-byte lines hold the role's matrix
    /// ([`MatrixLines`]); `None` for every other role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<MatrixLines>,
}
/// How a matrix constant's 16-byte lines hold its role's matrix, the one that maps column vectors (a view-projection's
/// clip position is `M * p`). Neither the layout's packing (`row_major`, `column_major`) nor the role decides it, since
/// a program may multiply on either side (`mul(M, v)`, `mul(v, M)`): the source states it from what the program does
/// (`iridia::matrix::lines`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixLines {
    /// Each line is one of the matrix's rows.
    Rows,
    /// Each line is one of its columns.
    Columns,
    /// The program does not show which, and why ([`Unshown`]). The role stands; a consumer that writes a matrix unequal
    /// to its transpose (a camera's) cannot bind it, one that writes the identity can.
    Unshown(Unshown),
}
/// Why a program does not show a matrix's lines ([`MatrixLines::Unshown`]): the kind a consumer branches on, its text
/// ([`std::fmt::Display`]) the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unshown {
    /// Its results are made of none of the matrix's words.
    NotRead,
    /// It reads them but sums no product of them with other values: it does not apply the matrix.
    NotApplied,
    /// It applies the matrix both ways: `along` sums of one line's words, `across` of one lane's.
    BothWays { along: usize, across: usize },
    /// The program could not be examined for it, and `why`: its value graph could not be built, or the matrix's buffer
    /// could not be found in it.
    Unexamined { why: String },
}
impl std::fmt::Display for Unshown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unshown::NotRead => f.write_str("the program's results are made of none of the matrix's words"),
            Unshown::NotApplied => f.write_str(
                "the program sums no product of the matrix's words with other values: it does not show how it \
                 applies the matrix",
            ),
            Unshown::BothWays { along, across } => write!(
                f,
                "the program applies the matrix both ways: {along} sums of one line's words and {across} of one lane's"
            ),
            Unshown::Unexamined { why } => f.write_str(why),
        }
    }
}
impl ConstantRole {
    /// Whether the role is a matrix, whose binding states its lines ([`ConstantBinding::lines`]): the view's and the
    /// object's transforms, their inverses and previous-frame and camera-relative forms, and the palettes.
    pub fn is_matrix(&self) -> bool {
        match self {
            ConstantRole::View(v) => matches!(
                v,
                ViewParameter::View
                    | ViewParameter::Projection
                    | ViewParameter::ViewProjection
                    | ViewParameter::CameraRelativeViewProjection
                    | ViewParameter::InverseView
                    | ViewParameter::InverseProjection
                    | ViewParameter::InverseViewProjection
                    | ViewParameter::PreviousViewProjection
            ),
            ConstantRole::Object(o) => matches!(
                o,
                ObjectParameter::World
                    | ObjectParameter::InverseWorld
                    | ObjectParameter::WorldViewProjection
                    | ObjectParameter::WorldView
                    | ObjectParameter::PreviousFrame(_)
                    | ObjectParameter::CameraRelative(_)
                    | ObjectParameter::CameraRelativePreviousFrame(_)
                    | ObjectParameter::BonePalette
                    | ObjectParameter::ClothCancelPalette
            ),
            ConstantRole::Material(_)
            | ConstantRole::Frame(_)
            | ConstantRole::AlphaReference { .. }
            | ConstantRole::Unknown { .. } => false,
        }
    }
}
/// How texels are filtered when a texture is magnified or minified (glTF's sampler filters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Filter {
    /// The nearest texel: crisp texels.
    Nearest,
    /// Bilinear between the four nearest texels.
    Linear,
}
/// How mip levels are used when a texture is minified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum MipFilter {
    /// No mip levels: the base level only (glTF's `NEAREST` or `LINEAR` minification filter).
    None,
    /// The nearest mip level.
    Nearest,
    /// Between the two nearest mip levels (trilinear with a linear `min`).
    Linear,
}
/// A sampler's filters as the source states them: each `None` where it states none (the renderer's default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Filters {
    /// Magnification.
    pub mag: Option<Filter>,
    /// Minification within a level.
    pub min: Option<Filter>,
    /// Minification across mip levels.
    pub mip: Option<MipFilter>,
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
    /// [`BufferTable::ObjectMatrices`] with one header element before each matrix: the same rows and row convention, `rows`
    /// rows of four floats after a header element of four words, so element `i` of the buffer is the header of matrix
    /// `i / (rows + 1)` when `i % (rows + 1)` is 0 and row `i % (rows + 1) - 1` of it otherwise.
    ///
    /// The program reads flags from the header's first word: its bit 0 with a 16-bit offset in bits 1 to 16, and its bit 18,
    /// each put the rows later by that many elements. A consumer writes every header as zero, which puts the rows directly
    /// after it. That the zero header is that layout is what a program run against such a table gave (identity rows after
    /// a zero header gave the position back), not something the engine documents.
    HeaderedMatrices { rows: u8 },
    /// The block of an instance whose program skins, in a table of elements of four floats each (the shape every one of
    /// the five skinned vertex programs of the hat's materials reads, measured by running each against tables filled to
    /// tell the parts apart):
    ///
    /// - a header element, as [`BufferTable::HeaderedMatrices`] (the instance record names it);
    /// - the first prefix element, `X`, `1 + offset` elements after the header, where `offset` is the header's bit 0 and
    ///   16-bit offset in bits 1 to 16 (zero when the bit is clear): its meaning is `first`;
    /// - the other prefix elements, `X + 1` to `X + prefix - 1`: none of the five programs reads them. A program that
    ///   reads one is not described by this role (a consumer fills them with zeros);
    /// - then the joints' matrices, `joint_stride` elements apart, joint `j`'s `rows` rows starting at element
    ///   `X + prefix + bit18 + joint_stride * j`, where `bit18` is the header's bit 18 (the rows are one element later
    ///   when it is set; `X` is not).
    ///
    /// The position of a vertex is the sum, over its influences, of the weight times the joint's matrix applied to the
    /// position (the rows as [`ObjectParameter::BonePalette`] has them), then `first`. A consumer writes a zero header
    /// (no offset, no bit 18), which puts `X` directly after the header and the first joint's rows `prefix` elements
    /// after `X`.
    SkinnedMatrices {
        /// Rows of one joint's matrix: three (an affine transform without its last row) or four.
        rows: u8,
        /// Elements from one joint's first row to the next joint's.
        joint_stride: u8,
        /// Elements from `X` to the first joint's first row when the header's bit 18 is clear.
        prefix: u8,
        /// What `X` holds.
        first: SkinPrefix,
    },
    /// One record per instance drawn, `stride` 32-bit words each, in instance order: word `matrix_start` of a record is
    /// the index, in elements of the table `matrices` (a [`BufferTable::ObjectMatrices`] or a
    /// [`BufferTable::HeaderedMatrices`] or [`BufferTable::SkinnedMatrices`] of the same stage), of the instance's first element:
    /// its first row, or its header for a headered table. The program adds the indices of the matrices it takes (a joint's, a model's own) to it.
    Instances {
        stride: u8,
        matrix_start: u8,
        matrices: ResourceId,
    },
}
/// What the first prefix element of a [`BufferTable::SkinnedMatrices`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SkinPrefix {
    /// The xyz lanes are added, once and unweighted, to the position the weighted sum of the joints' transforms gives
    /// (so a vertex at `p` with one influence of weight `w` ends at `w * (M p) + xyz`); the w lane is not read.
    Translation,
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
    /// Octahedral mapping in lanes 0 and 1: each stored as `e * stored.scale + stored.bias` (the producer's scale and
    /// bias, as measured) and read back by the engine as `v * read.scale + read.bias` before the unfold (`z = 1 - |x| -
    /// |y|`, x and y folded where z < 0, normalised). The two halves are stated as each side measures them and need not
    /// be exact inverses: an engine may store `0.4995 e + 0.4995` and read `2 v - 1`. The common 0-1 layout is `stored`
    /// [`LaneAffine::UNORM`] and `read` [`LaneAffine::SIGNED`].
    Octahedral { stored: LaneAffine, read: LaneAffine },
    /// Octahedral in the diamond layout over three 8-bit UNORM lanes: lanes 0 and 1 are the low eight bits of two 9-bit
    /// coordinates `u` and `v` (`round(255 (c + 1))`, 0 to 510 over -1 to 1), lane 2's bit 1 and bit 2 their ninth bits and
    /// its bit 0 the sign of z; the lane's other five bits are not part of the normal (a payload byte, which an engine
    /// may keep a class in). `x = (u + v) / 2`, `y = (u - v) / 2`, `|z| = 1 - |x| - |y|`, normalised
    /// ([`NormalEncoding::decode_octahedral_diamond`]).
    OctahedralDiamond,
    /// The normal by its major axis, over four lanes. The face is D3D's cube-map face of the axis whose component is
    /// largest in magnitude (`2 * axis + 1` where that component is negative: +x 0, -x 1, +y 2, -y 3, +z 4, -z 5), and
    /// lanes 0 and 1 are that face's `sc` and `tc` components of the normal, not divided by the major component (+x
    /// `(-z, -y)`, -x `(z, -y)`, +y `(x, z)`, -y `(x, -z)`, +z `(x, -y)`, -z `(-x, -y)`), each stored as `e *
    /// minor.scale + minor.bias`. Lane 2 is the major component's sign by its side of one half (below for a positive
    /// face, above for a negative one) and a payload `p` in 0 to 1 by its distance from one half, `p * payload.scale +
    /// payload.bias`; the payload is not part of the normal (a value an engine keeps beside it). Lane 3 is the axis
    /// over three (x 0, y 1/3, z 2/3). The major component's magnitude is the unit length's remainder
    /// ([`NormalEncoding::decode_cube_face`]).
    CubeFace { minor: LaneAffine, payload: LaneAffine },
}
/// One lane's affine map, `v * scale + bias`, by the f32 bits of each so that it compares and hashes exactly; it is written
/// as the numbers `scale` and `bias`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LaneAffine {
    pub scale_bits: u32,
    pub bias_bits: u32,
}
impl Serialize for LaneAffine {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("LaneAffine", 2)?;
        out.serialize_field("scale", &self.scale())?;
        out.serialize_field("bias", &self.bias())?;
        out.end()
    }
}
impl LaneAffine {
    /// A signed value stored in 0 to 1: `v * 0.5 + 0.5`.
    pub const UNORM: Self = Self {
        scale_bits: 0x3F00_0000,
        bias_bits: 0x3F00_0000,
    };

    /// A value in 0 to 1 read back as signed: `v * 2 - 1`.
    pub const SIGNED: Self = Self {
        scale_bits: 0x4000_0000,
        bias_bits: 0xBF80_0000,
    };

    pub fn new(scale: f32, bias: f32) -> Self {
        Self {
            scale_bits: scale.to_bits(),
            bias_bits: bias.to_bits(),
        }
    }

    pub fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }

    pub fn bias(&self) -> f32 {
        f32::from_bits(self.bias_bits)
    }

    /// `v * scale + bias`.
    pub fn apply(&self, v: f32) -> f32 {
        v * self.scale() + self.bias()
    }
}
/// The cube-map faces of [`NormalEncoding::CubeFace`] by face index: the axis of the major component and, for lanes 0
/// and 1, the axis and sign of the normal's component each holds.
pub(super) const CUBE_FACES: [(usize, [(usize, f64); 2]); 6] = [
    (0, [(2, -1.0), (1, -1.0)]),
    (0, [(2, 1.0), (1, -1.0)]),
    (1, [(0, 1.0), (2, 1.0)]),
    (1, [(0, 1.0), (2, -1.0)]),
    (2, [(0, 1.0), (1, -1.0)]),
    (2, [(0, -1.0), (1, -1.0)]),
];
impl NormalEncoding {
    /// The unit normal and the payload of four lanes stored as [`NormalEncoding::CubeFace`] with `minor` and
    /// `payload`; `steps` is each lane's quantisation step in the target's format (1/1023 for a 10-bit UNORM lane,
    /// 1/3 for a 2-bit one). Lane 3 is read as the nearest of 0, 1/3 and 2/3, and lane 2's side of one half as the
    /// major component's sign; a lane 3 not within half its step of one of them, or a lane 2 within one of its steps of
    /// one half, cannot be placed and is refused.
    pub fn decode_cube_face(
        minor: LaneAffine,
        payload: LaneAffine,
        lanes: [f32; 4],
        steps: [f32; 4],
    ) -> Result<([f64; 3], f32), String> {
        let [a, b, side, axis] = lanes;
        let thirds = f64::from(axis) * 3.0;
        let nearest = thirds.round();
        if !(0.0..=2.0).contains(&nearest) || (thirds - nearest).abs() / 3.0 >= f64::from(steps[3]) / 2.0 {
            return Err(format!(
                "a cube-face lane 3 of {axis} is not an axis (0, 1/3 or 2/3 within half a step of {})",
                steps[3]
            ));
        }
        let from_half = f64::from(side) - 0.5;
        if from_half.abs() < f64::from(steps[2]) {
            return Err(format!(
                "a cube-face lane 2 of {side} is within a step ({}) of one half: its sign cannot be read",
                steps[2]
            ));
        }
        let face = nearest as usize * 2 + usize::from(from_half > 0.0);
        let (major, components) = CUBE_FACES[face];
        let unmap = |v: f32| (f64::from(v) - f64::from(minor.bias())) / f64::from(minor.scale());
        let (sc, tc) = (unmap(a), unmap(b));
        let mut n = [0.0f64; 3];
        n[components[0].0] = sc * components[0].1;
        n[components[1].0] = tc * components[1].1;
        n[major] = (1.0 - sc * sc - tc * tc).max(0.0).sqrt() * if face % 2 == 1 { -1.0 } else { 1.0 };
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let p = ((from_half.abs() as f32 - payload.bias()) / payload.scale()).clamp(0.0, 1.0);
        Ok((n.map(|c| c / length), p))
    }

    /// The four lanes [`NormalEncoding::CubeFace`] with `minor` and `payload` stores the unit normal `normal` and the
    /// payload `p` (0 to 1) as, unquantised. The major axis is z where its magnitude is at least x's and y's, else y
    /// where it is at least x's, else x (on an edge between faces either face decodes to the normal).
    pub fn encode_cube_face(minor: LaneAffine, payload: LaneAffine, normal: [f32; 3], p: f32) -> [f32; 4] {
        let [x, y, z] = normal;
        let axis = if z.abs() >= x.abs() && z.abs() >= y.abs() {
            2
        } else if y.abs() >= x.abs() {
            1
        } else {
            0
        };
        let face = axis * 2 + usize::from(normal[axis] < 0.0);
        let (_, components) = CUBE_FACES[face];
        let lane = |(c, sign): (usize, f64)| minor.apply(if sign < 0.0 { -normal[c] } else { normal[c] });
        let offset = p * payload.scale() + payload.bias();
        [
            lane(components[0]),
            lane(components[1]),
            if face % 2 == 0 { 0.5 - offset } else { 0.5 + offset },
            axis as f32 / 3.0,
        ]
    }

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
/// How a scalar is stored in the lanes of an output: `stored = (value / scale)^(root / power)`, so the value is
/// `stored^(power / root) * scale` (the scale given by its f32 bits). Power and root 1 are linear; power 2 a square root
/// stored (a value kept in a perceptual range), 4 a fourth root; root 2 a square stored (a value the lighting takes the
/// square root of).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScalarEncoding {
    /// The power the stored value is raised to, to give the value over the scale: 1 for none, 2 for the square root
    /// stored, 4 for the fourth root. Never 0.
    pub power: u8,
    /// The root taken of the stored value raised to `power`: 1 for none, 2 for the square stored. Never 0.
    pub root: u8,
    /// The scale's f32 bits (1.0 for none): bits so the encoding compares and hashes exactly; it is written as the
    /// number `scale`.
    pub scale_bits: u32,
}
impl Serialize for ScalarEncoding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        // `root` is written only where it is not 1, so an encoding without one reads as it always has
        let mut out = serializer.serialize_struct("ScalarEncoding", 2 + usize::from(self.root != 1))?;
        out.serialize_field("power", &self.power)?;
        if self.root != 1 {
            out.serialize_field("root", &self.root)?;
        } else {
            out.skip_field("root")?;
        }
        out.serialize_field("scale", &f32::from_bits(self.scale_bits))?;
        out.end()
    }
}
impl ScalarEncoding {
    /// The value as it is (scale 1).
    pub const LINEAR: Self = Self {
        power: 1,
        root: 1,
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
            root: 1,
            scale_bits: scale.to_bits(),
        }
    }

    /// Stored as the square of the value over `scale`: the value is the stored one's square root times `scale`.
    pub fn square_of(scale: f32) -> Self {
        Self {
            power: 1,
            root: 2,
            scale_bits: scale.to_bits(),
        }
    }

    /// The value for a stored one (a root of a negative stored value is taken of 0).
    pub fn decode(&self, stored: f32) -> f32 {
        let raised = stored.powi(i32::from(self.power));
        let value = match self.root {
            0 | 1 => raised,
            2 => raised.max(0.0).sqrt(),
            root => raised.max(0.0).powf(1.0 / f32::from(root)),
        };
        value * f32::from_bits(self.scale_bits)
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutputRole {
    Albedo,
    Normal {
        encoding: NormalEncoding,
        space: NormalSpace,
    },
    /// Smoothness, 1 - perceptual roughness: the value decoded by `encoding`.
    Gloss {
        encoding: ScalarEncoding,
    },
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
    /// Specular reflectance at normal incidence (F0), the value decoded by `encoding`.
    Reflectance {
        encoding: ScalarEncoding,
    },
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
    /// Whether the fragment is kept: a value a discard of the program tests (the lane's written value is among what the
    /// discard's conditions are made of), so the surface is cut where the test fails. Not a fraction.
    Coverage,
    /// How opaque the surface is (1 opaque) as the target's blend takes it: the alpha lane of a target whose stated
    /// blend weighs the colour by the source alpha.
    Opacity,
    /// Lanes the program writes that no role is established for, with why: what the source's evidence shows of them
    /// (what reads them, or that nothing examined does). Never empty.
    Unknown {
        why: String,
    },
}
impl OutputRole {
    /// [`OutputRole::Unknown`] with `why`.
    pub fn unknown(why: impl Into<String>) -> OutputRole {
        OutputRole::Unknown { why: why.into() }
    }

    /// The encoding a role's value is stored in, for the roles that state one.
    pub fn scalar_encoding(&self) -> Option<ScalarEncoding> {
        match *self {
            OutputRole::Gloss { encoding }
            | OutputRole::Occlusion { encoding, .. }
            | OutputRole::Reflectance { encoding }
            | OutputRole::SpecularIntensity { encoding }
            | OutputRole::SpecularExponent { encoding }
            | OutputRole::Emission { encoding } => Some(encoding),
            _ => None,
        }
    }
}
impl ScalarEncoding {
    /// Whether the encoding is one: a power and a root other than 0, and a finite scale.
    pub fn check(&self) -> Result<(), String> {
        let scale = f32::from_bits(self.scale_bits);
        if self.power == 0 || self.root == 0 || !scale.is_finite() {
            return Err(format!(
                "an encoding of power {}, root {} and scale {scale}: power and root are never 0, the scale is finite",
                self.power, self.root
            ));
        }
        Ok(())
    }
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
/// What picks one of several stages of one kind in a binding: the mesh-side field a consumer reads, named by the
/// dialect's component and field that holds it (`<component>.<field>`), and the value that picks this stage. A
/// consumer draws a mesh with the stage whose key names a field it has and whose value that field equals; a mesh whose
/// value matches no stage is refused by name, not drawn with a first or default stage.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct VariantKey {
    /// The component and field the mesh carries the key in, as `component.field` (`<dialect>.<component>.<field>`), or
    /// the name the source gives its keys where no mesh field holds them (a name in the source's own terms).
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
/// What the source states of how it binds a constant buffer of the program, beyond what its constants are.
///
/// A program declares a constant buffer at the size it indexes it, and a source can bind more: D3D12 bounds a constant
/// buffer read by the size of the view the draw binds (the CBV's `SizeInBytes`), not by the shader's declaration, so a
/// skinning palette a program declares as two matrices is bound as the hundreds the draw uploads, and a joint index past
/// the declaration reads the palette, not zero. A lifter takes the rows of a buffer's uniform array from the bound size
/// when it has one (`iridia::BoundSizes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConstantBufferBinding {
    /// A constant buffer resource of the shader.
    pub resource: ResourceId,
    /// The bytes the source binds the buffer with, more than the program declares and a whole number of 16-byte rows;
    /// `None` is the declared size. Where the engine binds different sizes for different draws, the largest it binds:
    /// a consumer that fills fewer rows leaves the rest zero.
    pub bound_size: Option<u32>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An octahedral encoding states both of its maps, by bits, and writes them as numbers; the 0-1 layout's maps are
    /// the constants, and they invert each other.
    #[test]
    fn an_octahedral_encoding_states_its_stored_and_read_maps() {
        assert_eq!(LaneAffine::UNORM, LaneAffine::new(0.5, 0.5));
        assert_eq!(LaneAffine::SIGNED, LaneAffine::new(2.0, -1.0));
        for e in [-1.0f32, -0.25, 0.0, 0.5, 1.0] {
            assert_eq!(LaneAffine::SIGNED.apply(LaneAffine::UNORM.apply(e)), e);
        }
        let measured = NormalEncoding::Octahedral {
            stored: LaneAffine::new(0.4995, 0.4995),
            read: LaneAffine::SIGNED,
        };
        assert_ne!(
            measured,
            NormalEncoding::Octahedral {
                stored: LaneAffine::UNORM,
                read: LaneAffine::SIGNED,
            }
        );
        assert_eq!(
            serde_json::to_value(measured).unwrap(),
            serde_json::json!({"octahedral": {
                "stored": {"scale": 0.4995f32, "bias": 0.4995f32},
                "read": {"scale": 2.0, "bias": -1.0},
            }})
        );
    }
    /// The cube-face store as the source's G-buffer program writes it (`minor` 511/1023 both, `payload` 0.49804688 and
    /// 0.0014662757, the program's literals) gives the program's own output for the ten normals it was evaluated at
    /// with a payload of 0, and decodes back to them; every face, the edges and corners between faces and a grid of
    /// normals round-trip through 10-bit lanes and a 2-bit lane 3; a lane 3 that is not an axis, and a lane 2 within a
    /// step of one half, are refused.
    #[test]
    fn the_cube_face_decode_inverts_its_encoding_and_the_measured_lanes() {
        let k = 511.0f32 / 1023.0;
        let minor = LaneAffine::new(k, k);
        assert_eq!(minor.scale_bits, 0x3EFF_BFF0);
        let payload = LaneAffine::new(0.498_046_88, 0.001_466_275_7);
        let steps = [1.0 / 1023.0, 1.0 / 1023.0, 1.0 / 1023.0, 1.0 / 3.0];
        let encoding = NormalEncoding::CubeFace { minor, payload };
        assert_eq!(
            serde_json::to_value(encoding).unwrap()["cube_face"]["minor"]["scale"],
            serde_json::json!(k)
        );
        // the program's T0 for each normal (its TEXCOORD2), as the evaluation printed it
        let measured: [([f32; 3], [f32; 4]); 10] = [
            ([0.36, 0.48, 0.8], [0.679335, 0.259746, 0.498534, 0.666667]),
            ([0.36, 0.48, -0.8], [0.319687, 0.259746, 0.501466, 0.666667]),
            ([-0.48, 0.36, -0.8], [0.739277, 0.319687, 0.501466, 0.666667]),
            ([0.8, 0.36, 0.48], [0.259746, 0.319687, 0.498534, 0.0]),
            ([-0.8, 0.36, -0.48], [0.259746, 0.319687, 0.501466, 0.0]),
            ([0.36, 0.8, -0.48], [0.679335, 0.259746, 0.498534, 0.333333]),
            ([-0.48, -0.8, 0.36], [0.259746, 0.319687, 0.501466, 0.333333]),
            ([0.0, 0.0, -1.0], [0.499511, 0.499511, 0.501466, 0.666667]),
            ([0.0, 0.0, 1.0], [0.499511, 0.499511, 0.498534, 0.666667]),
            ([-0.6, 0.0, -0.8], [0.799218, 0.499511, 0.501466, 0.666667]),
        ];
        for (n, t0) in measured {
            let lanes = NormalEncoding::encode_cube_face(minor, payload, n, 0.0);
            assert!(
                lanes.iter().zip(&t0).all(|(a, b)| (a - b).abs() < 1e-6),
                "{n:?}: {lanes:?} against {t0:?}"
            );
            let (back, p) = NormalEncoding::decode_cube_face(minor, payload, t0, steps).unwrap();
            let dot: f64 = back.iter().zip(&n).map(|(a, b)| a * f64::from(*b)).sum();
            assert!(dot > 1.0 - 1e-6 && p < 1e-3, "{n:?}: {back:?} payload {p}");
        }
        let quantise = |lanes: [f32; 4]| {
            let q = |v: f32, levels: f32| (v * levels).round() / levels;
            [
                q(lanes[0], 1023.0),
                q(lanes[1], 1023.0),
                q(lanes[2], 1023.0),
                q(lanes[3], 3.0),
            ]
        };
        let round_trip = |n: [f64; 3], p: f32| {
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = n.map(|c| (c / length) as f32);
            let lanes = quantise(NormalEncoding::encode_cube_face(minor, payload, n, p));
            let (back, read) = NormalEncoding::decode_cube_face(minor, payload, lanes, steps).unwrap();
            let dot: f64 = back.iter().zip(&n).map(|(a, b)| a * f64::from(*b)).sum();
            (dot, (read - p).abs())
        };
        // the six faces, the twelve edges and the eight corners between them, with payloads 0, 1/2 and 1
        let mut worst = (1.0f64, 0.0f32);
        for x in [-1.0, 0.0, 1.0] {
            for y in [-1.0, 0.0, 1.0] {
                for z in [-1.0, 0.0, 1.0] {
                    if [x, y, z] == [0.0; 3] {
                        continue;
                    }
                    for p in [0.0, 0.5, 1.0] {
                        let (dot, off) = round_trip([x, y, z], p);
                        worst = (worst.0.min(dot), worst.1.max(off));
                    }
                }
            }
        }
        // and a grid over the sphere
        for i in 0..40 {
            for j in 0..80 {
                let (theta, phi) = (
                    std::f64::consts::PI * (f64::from(i) + 0.5) / 40.0,
                    std::f64::consts::TAU * f64::from(j) / 80.0,
                );
                let (dot, off) = round_trip(
                    [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()],
                    0.25,
                );
                worst = (worst.0.min(dot), worst.1.max(off));
            }
        }
        // a 10-bit step of a minor component is about 2/1023: the direction is within a few of them
        assert!(
            worst.0 > 1.0 - 2e-5 && worst.1 < 2.0 / 1023.0,
            "worst dot {}, payload off {}",
            worst.0,
            worst.1
        );
        let refused =
            |lanes: [f32; 4]| NormalEncoding::decode_cube_face(minor, payload, lanes, steps).unwrap_err();
        assert!(refused([0.5, 0.5, 0.49, 1.0]).contains("is not an axis"));
        // between two thirds in a lane finer than 2 bits (a 10-bit lane 3)
        let fine = [steps[0], steps[1], steps[2], 1.0 / 1023.0];
        assert!(
            NormalEncoding::decode_cube_face(minor, payload, [0.5, 0.5, 0.49, 0.5], fine)
                .unwrap_err()
                .contains("is not an axis")
        );
        assert!(
            NormalEncoding::decode_cube_face(minor, payload, [0.5, 0.5, 0.49, 341.0 / 1023.0], fine).is_ok()
        );
        assert!(refused([0.5, 0.5, 0.5 + 0.5 / 1023.0, 0.0]).contains("its sign cannot be read"));
    }
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
        // a square stored over 0.95: the value is the square root of the stored one, times 0.95; written with its root
        let square = ScalarEncoding::square_of(0.95);
        assert_eq!(square.decode(0.25), 0.475);
        assert_eq!(square.decode(-0.25), 0.0);
        assert_eq!(
            serde_json::to_value(square).unwrap(),
            serde_json::json!({"power": 1, "root": 2, "scale": 0.95f32})
        );
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
}
