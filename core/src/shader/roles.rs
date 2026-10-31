//! What each thing a shader program reads and writes stands for: input, constant, texture, buffer and output roles. It
//! states roles, never frozen values: whoever evaluates or renders the program (an offline baker, a renderer) decides
//! what a role is worth (a camera position, the time, the frame's exposure) and says so. The program's own vocabulary
//! (its interface, resources and render state) is `iridia_interface`'s; what its code shows of a constant or an output
//! (`MatrixLines`, `AlphaComparison`, `OutputUse`) is read by `iridia`'s analyses, and turned into roles here
//! ([`state_lines`], [`bind_output_uses`]).

use serde::Serialize;

use iridia_interface::{AlphaComparison, Label, MatrixLines, OutputUse, ResourceId};

use super::encoding::{LaneAffine, NormalEncoding, NormalSpace, ScalarEncoding};

/// A vertex attribute a program's input reads: what the drawn mesh's stream holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Attribute {
    /// The vertex position.
    Position,
    /// The vertex normal.
    Normal,
    /// Tangent set: xyz tangent, w the bitangent sign. Set 0 is glTF's standard TANGENT.
    Tangent(u8),
    /// u right, v down: (0, 0) is the image's top-left corner (glTF).
    TexCoord(u8),
    /// Vertex colour set `n`.
    Color(u8),
    /// Joint indices set `n` of a skinned mesh.
    Joints(u8),
    /// Joint weights set `n` of a skinned mesh.
    Weights(u8),
    /// Opaque per-vertex shader data at the game's input index, copied through geometry passes untransformed.
    /// A consumer that cannot preserve it must refuse ShaderInput by name.
    ShaderInput(u8),
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
    /// Lanes of vertex attributes as [`InputRole::Lanes`] states them, each component stored through `map` (`v * scale +
    /// bias` of the attribute's own value `v`) and read by the program as a float: a signed unit vector the source stores
    /// in unsigned normalized bytes is [`LaneAffine::UNORM`], which the program unpacks as `x * 2 - 1`. A lane no entry
    /// fills is not stated.
    Mapped {
        /// The attributes' components and the lanes they fill.
        lanes: Vec<LaneStream>,
        /// The map each component is stored through.
        map: LaneAffine,
    },
    /// Values of the drawn instance the source feeds through a vertex input, the same for every vertex of the instance:
    /// an instance stream the engine fills at draw time, not a stream of the mesh. Each entry is a value and the lanes it
    /// fills; a lane no entry fills is not stated. A consumer whose vertices already hold the instance's transform (a
    /// scene in world space) feeds the identity instance.
    Instance(Vec<InstanceLanes>),
    /// A corner of the triangle a full-screen step of a render graph draws, which covers the view: the value of the
    /// corner it holds. The three corners, in vertex order, are at the clip-space positions (-1, 1, 1, 1), (3, 1, 1, 1)
    /// and (-1, -3, 1, 1), with the texture coordinates (0, 0), (2, 0) and (0, 2): (0, 0) at the view's top-left
    /// corner, 1 a whole view to the right and down.
    ScreenTriangle(ScreenTriangleValue),
    /// Nothing known: the source's own meaning is not resolved, and why.
    Unknown {
        /// What was examined and why no role follows.
        why: String,
    },
}
/// What an input of a full-screen triangle holds of each corner ([`InputRole::ScreenTriangle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenTriangleValue {
    /// The corner's clip-space position (four lanes).
    Position,
    /// The corner's texture coordinate (two lanes).
    TextureCoordinate,
}
/// The lanes `first..first + count` of an input: `count` components of the vertex attribute `semantic`, from component
/// `component` (the attribute's own components: x 0, y 1, z 2, w 3), each in the encoding the input's scalar type has, as
/// the other packed inputs take theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LaneStream {
    /// The vertex attribute the lanes come from.
    pub semantic: Attribute,
    /// The first lane (0 for x).
    pub first: u8,
    /// How many lanes, starting at `first`, it fills.
    pub count: u8,
    /// The first component of the attribute lane `first` takes (0 for all but the programs that take one component of an
    /// attribute apart: a source game's cloth programs read the tangent's w, the bitangent sign, as the one
    /// byte of an input named BINORMAL0, as `(byte - 127) / 127` where the other permutations read TANGENT0.w; inferred
    /// from the program's decode of it, not captured from the game).
    pub component: u8,
}
/// The lanes `first..first + count` of an input that hold one value of the drawn instance ([`InputRole::Instance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct InstanceLanes {
    /// The value.
    pub value: InstanceValue,
    /// The first lane (0 for x).
    pub first: u8,
    /// How many lanes, from `first`, it fills: the value's own components, from its first.
    pub count: u8,
}
/// A value of the drawn instance an instance stream carries ([`InputRole::Instance`]). The program places a vertex at
/// `translation + scale * (axis 0, axis 1, axis 2) * p` for the position `p` it takes from the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InstanceValue {
    /// The instance's translation in world space (three components).
    Translation,
    /// The instance's uniform scale (one component).
    Scale,
    /// Axis `n` of the instance's rotation (0 x, 1 y, 2 z) in world space, a unit vector (three components).
    Axis(u8),
    /// How far the instance's geometry is blended from the coarser level of detail the mesh also stores toward its own
    /// position (one component): 1 draws the mesh's own position, 0 the coarser one.
    LodBlend,
}
/// A camera or view-wide value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ViewParameter {
    /// The camera's position in world space (xyz).
    CameraPosition,
    /// The world-to-view matrix.
    View,
    /// The view-to-clip projection matrix.
    Projection,
    /// The world-to-clip matrix (view times projection).
    ViewProjection,
    /// The view-projection of a world with the camera at its origin: a program multiplies a world position minus the
    /// camera's ([`ViewParameter::CameraPosition`]) by it, so it is [`ViewParameter::ViewProjection`] without the camera's
    /// translation. The program takes the camera position from its own constant, so the two roles are bound together; a
    /// consumer that feeds the plain matrix here puts the geometry in the wrong place.
    CameraRelativeViewProjection,
    /// The view-to-world matrix.
    InverseView,
    /// The clip-to-view matrix.
    InverseProjection,
    /// The clip-to-world matrix.
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
    /// The part of the render target the view draws into, per axis: its drawn size over the target's size, two values. A
    /// renderer that lowers its resolution from frame to frame draws into that part of a target of the full size, so a
    /// program finds its texture coordinate on the screen as the pixel position times
    /// [`ViewParameter::InverseViewportSize`] over this; a view that fills its target has 1, 1.
    ResolutionScale,
    /// Whether the view's projection is a perspective one, as an unsigned 32-bit integer word: 1 for a perspective
    /// projection (clip w is the depth along the view axis), 0 for an orthographic one.
    Perspective,
    /// The far clip plane's distance minus the near clip plane's ([`ViewParameter::FarPlane`] minus
    /// [`ViewParameter::NearPlane`]), in world units (one value).
    DepthRange,
    /// The device depth a perspective projection gives a point infinitely far away: a point at depth `d` along the view
    /// axis (world units) gets the device depth `offset + scale / d` ([`ViewParameter::DeviceDepthScale`] is `scale`),
    /// so a program takes the depth back as `scale / (device depth - offset)`. One value.
    DeviceDepthOffset,
    /// The scale of a perspective projection's device depth over the depth along the view axis: `scale` in
    /// [`ViewParameter::DeviceDepthOffset`]'s `offset + scale / d`. One value.
    DeviceDepthScale,
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
    /// The world-to-object matrix.
    InverseWorld,
    /// The object-to-clip matrix.
    WorldViewProjection,
    /// The object-to-view matrix.
    WorldView,
    /// The view-to-object matrix: inverse(object-to-world) * inverse(world-to-view).
    ViewToObject,
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
    /// skinning program blends by the vertex's joint indices and weights. Where the program also reads
    /// [`ObjectParameter::ClothCancelPalette`], a joint's transform is the product of the two; a consumer that has
    /// each joint's whole object-to-world transform states it here and the identity there.
    BonePalette,
    /// The matrices of a second palette a vertex program takes the vertex through before
    /// [`ObjectParameter::BonePalette`] (the engine's name for it is the cloth-cancel matrices): each a 3 x 4
    /// transform held as the binding states ([`MatrixLines`]), chosen by the same index as the bone palette's (the
    /// vertex's joint index plus the instance's matrix base, [`ObjectParameter::MatrixBase`]), so as long as it. The
    /// vertex's transform is `BonePalette[j] * ClothCancelPalette[j]`: the position, and the normal and tangent by
    /// the 3 x 3 part, go through this matrix first (blended over the vertex's joints by their weights where the
    /// program blends joints, as Elden Ring's character programs that skin do; those that take one joint per vertex
    /// do the same with that joint). How an engine splits a joint's transform between the two palettes is not
    /// established: in a captured Elden Ring frame, 3,746 of the 12,230 recorded entries of skinned draws' palettes
    /// were the identity, and 7,940 were not. A consumer that states each joint's whole transform in the bone palette
    /// feeds the identity here, which leaves the first step without effect.
    ClothCancelPalette,
    /// The instance's environment data word (an unsigned integer the engine writes per instance; the layered programs
    /// scale it by 1/255 into the alpha lane of a G-buffer target, and a program that reads a buffer of light sets takes
    /// the set it names).
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
    /// carries ([`ObjectParameter::Fade`]).
    DitherAlphaEnabled,
    /// The second per-model flag of the dithered transparency, set together with [`ObjectParameter::DitherAlphaEnabled`]
    /// (a program reads both, and fades only where both are on).
    DitherEnabled,
    /// The factor the engine fades an instance's visibility by (one value): 1 where it is fully shown, 0 where it is gone. A
    /// program applies it where its pass keeps the instance's visibility: multiplied into the alpha it writes or tests (an
    /// alpha test, also one steered through the depth output), stored as the alpha of the image a pass writes, as the weight
    /// of the surface against the scene behind it where the program blends itself, and, in a pass that has no alpha, into the
    /// emission it writes, or, under the dithered transparency's flags ([`ObjectParameter::DitherAlphaEnabled`]), as the
    /// share of fragments a dithered test keeps (a fragment survives where the dither noise less one half plus this factor
    /// reaches one half). A consumer that draws the instance as fully shown feeds 1. The camera fade and the dither flags
    /// are other mechanisms: they switch the discard by noise on, they do not carry the factor.
    Fade,
    /// Whether the wind leaves the instance's vertices still (one value): above zero, the program places each vertex where
    /// its rest position and the instance put it; at zero or below, it moves it by the wind state the program's other
    /// wind constants carry. A consumer that draws the vegetation still states this and says so; one that animates it
    /// needs those constants as well.
    WindOff,
}
/// A value of the frame being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FrameParameter {
    /// Seconds since a start the game defines.
    Time,
    /// The hour of the day the scene shows, one value, 0 to 24: the world's [`crate::TimeOfDay`]. A program of a source
    /// that keys values by the hour reads it (Elden Ring's `FC_TimeOfDay`, which its sky programs take modulo 24 to blend
    /// their material's colours by the hour). A world that states no hour gives such a program no value: a consumer
    /// refuses the draw rather than show one hour as another.
    TimeOfDay,
    /// The pre-exposure: the factor the renderer multiplies the radiance it writes into the frame's light targets by (a
    /// light's contribution, the ambient, a material's emission), so that the targets hold radiance times this factor; 1
    /// where a renderer does not pre-expose. What the frame's exposure and tone stages do with it is theirs: they are passes
    /// of their own, with their own bindings. A program that multiplies its emission by this value writes pre-exposed
    /// radiance; a consumer that lights in unscaled radiance and exposes once at the end gives it 1.
    PreExposure,
    /// One over [`FrameParameter::PreExposure`]: the factor a program multiplies a value it reads back from the frame's
    /// light targets by to take the pre-exposure off it. One value; 1 where the pre-exposure is 1.
    InversePreExposure,
    /// The frame's exposure: the factor its exposure stage multiplies the scene's radiance by before the tone curve (a
    /// renderer's adapted exposure). One value; 1 where the frame is not exposed.
    Exposure,
    /// One over [`FrameParameter::Exposure`]: a program that multiplies a colour by it puts that colour on the screen at
    /// the same brightness whatever the exposure. One value.
    InverseExposure,
    /// An index into a palette of colours a draw selects from.
    ColourSlot,
    /// The direction the dominant directional light travels (xyz): a surface is lit by `saturate(dot(normal, -xyz))`.
    LightDirection,
    /// The unit direction from the scene towards the dominant directional light (xyz): the opposite of
    /// [`FrameParameter::LightDirection`], so a surface is lit by `saturate(dot(normal, xyz))`.
    DirectionToLight,
    /// The colour (rgb) of that light as the source's programs read it: the scene's sun's (`core.environment`'s)
    /// `color` times its `intensity` over the world's [`crate::DirectionalLightUnit::lux_per_unit`], the value the
    /// source stored. A world with no sun or no such unit gives a consumer no value of it.
    LightColour,
    /// The colour (rgb) of that light's highlights as the source's programs read it: as [`FrameParameter::LightColour`],
    /// of the sun's `specular` colour and intensity where it states them ([`crate::LightSpecular`]), of its own
    /// otherwise.
    LightSpecularColour,
    /// The directional ambient light as four values: its colour (xyz: a lighting program adds each channel to the same
    /// channel of the lit colour, scaled by how much the surface faces an ambient direction it also reads), and a w that
    /// scales the ambient terms a vertex carries, growing from 1 (no change) with distance.
    DirectionalAmbientColour,
    /// How many records the frame's light table of this kind holds ([`BufferSource::Lights`]): one unsigned 32-bit
    /// integer, the number of lights the consumer packed ([`super::PackedLights::lights`]).
    LightCount(super::LightTableKind),
    /// The side of a light-list tile in pixels ([`BufferSource::LightLists`]): one unsigned 32-bit integer, the size the
    /// consumer packed its lists for.
    LightTileSize,
    /// How many light-list tiles a row of the screen holds: one unsigned 32-bit integer, the render target's width over
    /// [`FrameParameter::LightTileSize`], rounded up.
    LightTilesPerRow,
}
/// What a constant of a buffer stands for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstantRole {
    /// The material's own parameter, as stored: the 32-bit words the constant takes (bit patterns, so an integer
    /// parameter and a float one are both exact).
    Material(Vec<u32>),
    /// A camera or view-wide value.
    View(ViewParameter),
    /// A value of the drawn object.
    Object(ObjectParameter),
    /// A value of the frame.
    Frame(FrameParameter),
    /// A reference confirmed by the program's discard graph. `compared` states whether a portable cutoff on output
    /// alpha is the same test or an approximation. A baking consumer freezes the reference below the alpha's range, and must
    /// also neutralize any stated dither perturbation; the draw uses the material's original `value`.
    AlphaReference {
        /// The reference the discard compares against.
        value: f32,
        /// What the reference is compared with.
        compared: AlphaComparison,
    },
    /// Not resolved to a role, and why.
    Unknown {
        /// What was examined and why no role follows.
        why: String,
    },
    /// A value of the engine's that no role of core states, set by the source's own policy: the 32-bit words the
    /// constant takes, and why (as [`super::TextureSource::Default`] sets a texture slot the engine fills). Not the
    /// material's: a consumer writes the words as they are.
    Default {
        /// The words, as [`ConstantRole::Material`]'s.
        words: Vec<u32>,
        /// The policy and the evidence for its words.
        why: String,
    },
}
/// One constant: the bytes `offset..offset + size` of a constant buffer resource of the shader, named when the
/// source names it (a stripped program has fields by offset only), and what it stands for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConstantBinding {
    /// The constant buffer resource holding the constant.
    pub resource: ResourceId,
    /// Byte offset in the buffer.
    pub offset: u32,
    /// Bytes.
    pub size: u32,
    /// The constant's name, `None` for a stripped program.
    pub name: Option<String>,
    /// What the constant stands for.
    pub role: ConstantRole,
    /// For a matrix role ([`ConstantRole::is_matrix`]), how its 16-byte lines hold the role's matrix
    /// ([`MatrixLines`]); `None` for every other role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<MatrixLines>,
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
                    | ObjectParameter::ViewToObject
                    | ObjectParameter::PreviousFrame(_)
                    | ObjectParameter::CameraRelative(_)
                    | ObjectParameter::CameraRelativePreviousFrame(_)
                    | ObjectParameter::BonePalette
                    | ObjectParameter::ClothCancelPalette
            ),
            ConstantRole::Material(_)
            | ConstantRole::Frame(_)
            | ConstantRole::AlphaReference { .. }
            | ConstantRole::Unknown { .. }
            | ConstantRole::Default { .. } => false,
        }
    }
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
    /// Image `n` of the decal buffer the frame's decal step draws before the surfaces are lit (Unity's DBuffer: 0 the
    /// decals' albedo, 1 their normal, 2 their metallic, occlusion and smoothness, each with the decals' coverage in
    /// alpha): a lit draw reads it at its own screen position and blends the decals into its surface.
    DecalBuffer(u8),
}
impl FrameTexture {
    /// The dimension of the image: a probe's is a cube, the light probes' a volume, every other a plane.
    pub fn dim(self) -> iridia_interface::TextureDim {
        match self {
            Self::ReflectionProbe => iridia_interface::TextureDim::Cube,
            Self::LightProbeVolume => iridia_interface::TextureDim::D3,
            Self::SceneDepth
            | Self::SceneColour
            | Self::ShadowMap
            | Self::ScreenSpaceShadows
            | Self::AmbientOcclusion
            | Self::DecalBuffer(_) => iridia_interface::TextureDim::D2,
        }
    }
}
/// One buffer slot: a buffer resource of the shader (a structured, raw or typed buffer a program indexes at run time) and
/// what the source binds to it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BufferBinding {
    /// The buffer resource of the shader.
    pub resource: ResourceId,
    /// What the source binds to it.
    pub source: BufferSource,
}
/// What a buffer slot holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferSource {
    /// A table of the draw's own data, by what it holds: the consumer fills it from the draw it makes.
    Table(BufferTable),
    /// Nothing is known of the contents and the source says why: a consumer states the stand-in it uses, and never
    /// reads the buffer as zeros without saying so (as a texture with no known source, or an unknown
    /// constant).
    Unknown {
        /// Why nothing is known of the contents.
        why: String,
    },
    /// One of the frame's own buffers: only in the binding of a step of the frame's `core.render_graph`
    /// ([`crate::render_graph::RenderGraph::buffers`]), whose buffers these are. A material's binding never states one.
    Graph(crate::render_graph::BufferId),
    /// The scene's lights of one kind, one record each in the table's layout, which the consumer packs from the world's
    /// `core.light`s for the frame's view ([`super::LightTable::pack`]).
    Lights(super::LightTable),
    /// One of the two buffers of the frame's per-tile light lists, which say which lights a program visits at a pixel;
    /// the consumer packs both with [`super::pack_light_lists`] from the lights it packed into the stage's light tables.
    LightLists(super::LightListPart),
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
    ObjectMatrices {
        /// Rows of four floats per matrix: 3 or 4.
        rows: u8,
    },
    /// [`BufferTable::ObjectMatrices`] with one header element before each matrix: the same rows and row convention, `rows`
    /// rows of four floats after a header element of four words, so element `i` of the buffer is the header of matrix
    /// `i / (rows + 1)` when `i % (rows + 1)` is 0 and row `i % (rows + 1) - 1` of it otherwise.
    ///
    /// The program reads flags from the header's first word: its bit 0 with a 16-bit offset in bits 1 to 16, and its bit 18,
    /// each put the rows later by that many elements. A consumer writes every header as zero, which puts the rows directly
    /// after it. That the zero header is that layout is what a program run against such a table gave (identity rows after
    /// a zero header gave the position back), not something the engine documents.
    HeaderedMatrices {
        /// Rows of four floats after each header element: 3 or 4.
        rows: u8,
    },
    /// The block of an instance whose program skins, in a table of elements of four floats each (the shape every one of
    /// the five skinned vertex programs of one game's hat materials reads, measured by running each against tables filled to
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
        /// 32-bit words in each record.
        stride: u8,
        /// Index of the word of a record that holds the index of the instance's first matrix element.
        matrix_start: u8,
        /// The matrix table (`ObjectMatrices`, `HeaderedMatrices` or `SkinnedMatrices`) the indices point into.
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
    /// The surface's albedo (base colour), linear.
    Albedo,
    /// The surface normal, decoded by `encoding` into `space`.
    Normal {
        /// How the normal is stored in the lanes.
        encoding: NormalEncoding,
        /// The space the normal is in.
        space: NormalSpace,
    },
    /// Smoothness, 1 - perceptual roughness: the value decoded by `encoding`.
    Gloss {
        /// How the gloss is stored.
        encoding: ScalarEncoding,
    },
    /// The perceptual roughness, stored as is.
    Roughness,
    /// The metalness (0 to 1).
    Metallic,
    /// The factor the light `of` is scaled by where the surface is shadowed from it (1 unoccluded, 0 fully): the value
    /// decoded by `encoding`, as the lighting pass multiplies the light by it.
    Occlusion {
        /// Which light the occlusion applies to.
        of: Occluded,
        /// How the factor is stored.
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
        /// How the reflectance is stored.
        encoding: ScalarEncoding,
    },
    /// The strength of the specular term, how much light the surface reflects as a highlight (0 to 1 once decoded).
    SpecularIntensity {
        /// How the strength is stored.
        encoding: ScalarEncoding,
    },
    /// How tight the specular highlight is: the material's falloff or exponent as the source's lighting uses it
    /// (what lighting derives from it, a Blinn-Phong exponent for instance, is the source's).
    SpecularExponent {
        /// How the exponent is stored.
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
        /// How the emission is stored.
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
        /// What the source's evidence shows of the lanes.
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
/// Lanes of one output varying and their role: an output's lanes may carry different things (albedo in rgb and
/// occlusion in a), as several bindings of one label.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputBinding {
    /// The output varying the lanes belong to.
    pub label: Label,
    /// Lane mask: bit k is lane k of the output's value.
    pub lanes: u8,
    /// What the lanes stand for.
    pub role: OutputRole,
}
/// A stage's input binding: the role of an input varying the shader declares.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputBinding {
    /// The input varying.
    pub label: Label,
    /// What the input stands for.
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
    /// The value of the stage's key, compared for equality with the mesh's field.
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
    /// The part's name.
    pub name: String,
    /// The part's value.
    pub value: u64,
}

/// States the lines ([`ConstantBinding::lines`]) of each matrix role among `constants`, as `lines_at` reads them from the
/// program (`iridia::matrix::lines_at` over the program's value graph); every other role is left alone. Returns how many
/// are [`MatrixLines::Unshown`].
pub fn state_lines(
    constants: &mut [ConstantBinding],
    mut lines_at: impl FnMut(&ConstantBinding) -> MatrixLines,
) -> usize {
    let mut unshown = 0usize;
    for c in constants.iter_mut().filter(|c| c.role.is_matrix()) {
        let lines = lines_at(c);
        if matches!(lines, MatrixLines::Unshown(_)) {
            unshown = unshown.saturating_add(1);
        }
        c.lines = Some(lines);
    }
    unshown
}

/// The lanes [`bind_output_uses`] bound, for a log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputUses {
    /// Lanes bound as `OutputRole::Coverage`.
    pub coverage: u32,
    /// Lanes bound as `OutputRole::Opacity`.
    pub opacity: u32,
}

/// Bind the Unknown lanes of a pixel stage's `outputs` that what its program shows of them (`uses`, from
/// `iridia::output_uses::output_uses`) gives a use for, by the output's label:
///
/// - [`OutputRole::Opacity`], a lane the target's stated blend weighs the colour by ([`OutputUse::blend_weight`]: the
///   alpha lane of a target blended by the source alpha);
/// - else [`OutputRole::Coverage`], a lane a discard tests ([`OutputUse::discard_tested`]).
///
/// A lane with a role keeps it, and an Unknown binding keeps its why on the lanes not bound. A caller that states roles
/// for the outputs of a program calls it after stating its own; nothing is read from a program's or a target's name.
pub fn bind_output_uses(outputs: &mut Vec<OutputBinding>, uses: &[OutputUse]) -> OutputUses {
    let mut counts = OutputUses::default();
    let mut bound = Vec::with_capacity(outputs.len());
    for o in outputs.drain(..) {
        let OutputRole::Unknown { why } = &o.role else {
            bound.push(o);
            continue;
        };
        let Some(seen) = uses.iter().find(|u| u.label == o.label) else {
            bound.push(o);
            continue;
        };
        let opacity = o.lanes & seen.blend_weight;
        let coverage = o.lanes & seen.discard_tested & !opacity;
        let rest = o.lanes & !(opacity | coverage);
        if rest != 0 {
            bound.push(OutputBinding {
                label: o.label.clone(),
                lanes: rest,
                role: OutputRole::unknown(why.clone()),
            });
        }
        for (lanes, role) in [(coverage, OutputRole::Coverage), (opacity, OutputRole::Opacity)] {
            if lanes != 0 {
                bound.push(OutputBinding {
                    label: o.label.clone(),
                    lanes,
                    role,
                });
            }
        }
        counts.coverage = counts.coverage.saturating_add(coverage.count_ones());
        counts.opacity = counts.opacity.saturating_add(opacity.count_ones());
    }
    *outputs = bound;
    counts
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked values")]
mod tests {
    use super::*;

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

    fn target(n: u32) -> Label {
        Label {
            name: "SV_Target".into(),
            index: n,
        }
    }

    fn seen(n: u32, blend_weight: u8, discard_tested: u8) -> OutputUse {
        OutputUse {
            label: target(n),
            target: n,
            blend_weight,
            discard_tested,
        }
    }

    /// A lane the blend weighs by is Opacity, over a discard's test of it; a tested lane is Coverage; a lane with a
    /// role keeps it; an Unknown binding keeps its why on the lanes not bound; an output nothing is seen of is left.
    #[test]
    fn the_uses_seen_bind_the_unknown_lanes() {
        let unknown = |n: u32, lanes: u8| OutputBinding {
            label: target(n),
            lanes,
            role: OutputRole::unknown(format!("target {n}: no role")),
        };
        let mut outputs = vec![
            OutputBinding {
                label: target(0),
                lanes: 0b0111,
                role: OutputRole::Albedo,
            },
            unknown(0, 0b1000),
            unknown(1, 0b1111),
            unknown(2, 0b1000),
            unknown(3, 0b1111),
        ];
        let uses = [
            // the albedo's lanes are tested too: it keeps its role
            seen(0, 0, 0b1111),
            seen(1, 0b1000, 0b1001),
            seen(2, 0, 0),
        ];
        let counts = bind_output_uses(&mut outputs, &uses);
        let roles: Vec<_> = outputs
            .iter()
            .map(|o| (o.label.index, o.lanes, o.role.clone()))
            .collect();
        assert_eq!(
            roles,
            [
                (0, 0b0111, OutputRole::Albedo),
                (0, 0b1000, OutputRole::Coverage),
                (1, 0b0110, OutputRole::unknown("target 1: no role")),
                (1, 0b0001, OutputRole::Coverage),
                (1, 0b1000, OutputRole::Opacity),
                (2, 0b1000, OutputRole::unknown("target 2: no role")),
                (3, 0b1111, OutputRole::unknown("target 3: no role")),
            ]
        );
        assert_eq!(
            counts,
            OutputUses {
                coverage: 2,
                opacity: 1
            }
        );
    }

    /// Each matrix role is given the lines read for it, the others none; the unshown are counted.
    #[test]
    fn the_lines_of_each_matrix_role_are_stated() {
        let constant = |offset: u32, role: ConstantRole| ConstantBinding {
            resource: ResourceId(0),
            offset,
            size: 64,
            name: None,
            role,
            lines: None,
        };
        let mut constants = vec![
            constant(0, ConstantRole::View(ViewParameter::ViewProjection)),
            constant(64, ConstantRole::Object(ObjectParameter::BonePalette)),
            constant(128, ConstantRole::View(ViewParameter::CameraPosition)),
            constant(144, ConstantRole::View(ViewParameter::InverseView)),
        ];
        let unshown = state_lines(&mut constants, |c| match c.offset {
            0 => MatrixLines::Columns,
            64 => MatrixLines::Rows,
            _ => MatrixLines::Unshown(iridia_interface::Unshown::NotApplied),
        });
        assert_eq!(unshown, 1);
        let lines: Vec<_> = constants.iter().map(|c| c.lines.clone()).collect();
        assert_eq!(
            lines,
            [
                Some(MatrixLines::Columns),
                Some(MatrixLines::Rows),
                None,
                Some(MatrixLines::Unshown(iridia_interface::Unshown::NotApplied)),
            ]
        );
    }
}
