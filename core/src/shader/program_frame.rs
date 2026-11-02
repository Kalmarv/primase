//! The coordinate frame a source's programs compute in ([`ProgramFrame`]), and how each value a program reads or writes
//! moves between it and the world's frame ([`SpatialKind`]): one table over every role, so a consumer that runs the
//! source's programs on the world converts each value by its kind at the one place it binds them, and a new role
//! states its kind where it is added.

use serde_json::{Value, json};

use super::TextureSource;
use super::encoding::NormalSpace;
use super::lights::{LightListPart, LightValue};
use super::roles::{
    Attribute, BufferSource, BufferTable, ConstantRole, FrameParameter, FrameTexture, InstanceValue,
    ItemParameter, ObjectMatrix, ObjectParameter, OutputRole, ViewParameter,
};
use crate::frame::SourceFrame;
use crate::json::JsonError;
use crate::world::{Resource, World};

/// `core.program_frame`: the coordinate frame the source's game programs compute in, stated by the reader that keeps
/// them. The world is in core's frame ([`crate::frame`]); a source's programs compute in the frame their game's data is
/// in, which its reader converted the world out of. A consumer that runs them on the world converts every value they
/// read into their frame by its kind ([`SpatialKind`]) and reads what they write in it, so the programs compute as they
/// do in the game. There is no default: a reader states one form or the other, an identity included
/// (`Source(SourceFrame::WORLD)` for programs that compute in core's frame).
#[derive(Clone, Debug, PartialEq)]
pub enum ProgramFrame {
    /// The programs compute in this frame: `world = frame(program)`, as the reader converted the source's data.
    Source(SourceFrame),
    /// The frame the programs compute in is not established for this source, or a consumer's conversion into it is
    /// not checked: a consumer feeds them world values unconverted and shows `why` with every view of them. `why` names
    /// the world, what is known of its frame and on what evidence, and what is missing.
    NotConverted {
        /// The world, what is known of its programs' frame (with its evidence), and what is not.
        why: String,
    },
}

impl ProgramFrame {
    /// The frame values are converted into, where one is stated.
    pub fn frame(&self) -> Option<&SourceFrame> {
        match self {
            ProgramFrame::Source(frame) => Some(frame),
            ProgramFrame::NotConverted { .. } => None,
        }
    }
}

impl Resource for ProgramFrame {
    const NAME: &'static str = "core.program_frame";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(match self {
            ProgramFrame::Source(frame) => json!({
                "source": {
                    "axes": frame.axes().map(|a| format!("{a:?}")),
                    "metres_per_unit": frame.metres_per_unit(),
                }
            }),
            ProgramFrame::NotConverted { why } => json!({ "not_converted": why }),
        }))
    }
    /// A frame that is not converted says why.
    fn verify(&self, _w: &World) -> Result<(), String> {
        match self {
            ProgramFrame::Source(_) => Ok(()),
            ProgramFrame::NotConverted { why } if why.trim().is_empty() => {
                Err("a program frame that is not converted must say why".into())
            }
            ProgramFrame::NotConverted { .. } => Ok(()),
        }
    }
}

/// How a value moves from the world's frame into a program frame `C` (`world = C(program)`, a signed axis permutation
/// times a unit scale): the methods of [`SourceFrame`] that convert each kind are named with it. View space and clip
/// space are not frames of the world: a world-to-view matrix converted as [`SpatialKind::FromWorld`] takes the program's
/// positions to the same view space, so values in view or clip space, and lengths measured there, keep theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpatialKind {
    /// No coordinates of the world's space (a colour, a count, a texture coordinate, a view-space value): fed as it is.
    None,
    /// A position, or an offset between positions, in the world's or an object's space:
    /// [`SourceFrame::point_to_source`].
    Point,
    /// A direction or a normal: [`SourceFrame::direction_to_source`].
    Direction,
    /// A tangent, xyz a direction and w the bitangent's sign: [`SourceFrame::tangent_to_source`].
    Tangent,
    /// A plane `(n, d)` of the world's space: [`SourceFrame::plane_to_source`].
    Plane,
    /// A distance measured in the world's space: [`SourceFrame::length_to_source`].
    Length,
    /// One over a distance measured in the world's space: [`SourceFrame::inverse_length_to_source`].
    InverseLength,
    /// A matrix that takes world (or object) positions to a space that is not the world's (the view, the clip):
    /// [`SourceFrame::from_world_matrix_to_source`].
    FromWorld,
    /// A matrix that gives world (or object) positions from a space that is not the world's:
    /// [`SourceFrame::to_world_matrix_to_source`].
    ToWorld,
    /// A matrix from one space of the world's frame to another (an object's to the world, a joint's to its model's):
    /// [`SourceFrame::world_matrix_to_source`].
    WithinWorld,
    /// One column of a [`SpatialKind::WithinWorld`] rotation (an instance's axis): the column of the converted matrix,
    /// which for a permuting frame is another axis's column converted, so a consumer converts the whole rotation.
    WithinWorldColumn,
    /// A record of a light table: each field by its own kind ([`LightValue::spatial_kind`]).
    LightRecord,
    /// A cube image a program samples by a direction (a reflection probe): its faces must be the program frame's.
    CubeByDirection,
    /// A volume image a program samples at a position (light probes): its placement must be the program frame's.
    VolumeByPosition,
    /// An image whose texels hold directions (a decal normal buffer): written and read in the program frame.
    ImageOfDirections,
}

impl Attribute {
    /// How the stream moves into a program frame. The reader converted the mesh's positions, normals, tangents and
    /// points into the world's frame; the other streams hold no coordinates of it.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            Attribute::Position | Attribute::Point(_) => SpatialKind::Point,
            Attribute::Normal => SpatialKind::Direction,
            Attribute::Tangent(_) => SpatialKind::Tangent,
            Attribute::TexCoord(_)
            | Attribute::Color(_)
            | Attribute::Joints(_)
            | Attribute::Weights(_)
            | Attribute::ShaderInput(_) => SpatialKind::None,
        }
    }
}

impl InstanceValue {
    /// How the instance's value moves into a program frame.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            InstanceValue::Translation => SpatialKind::Point,
            InstanceValue::Axis(_) => SpatialKind::WithinWorldColumn,
            InstanceValue::Scale | InstanceValue::LodBlend => SpatialKind::None,
        }
    }
}

impl ViewParameter {
    /// How the view's value moves into a program frame. View and clip space keep theirs.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            ViewParameter::CameraPosition => SpatialKind::Point,
            ViewParameter::View
            | ViewParameter::ViewProjection
            | ViewParameter::CameraRelativeViewProjection
            | ViewParameter::PreviousViewProjection => SpatialKind::FromWorld,
            ViewParameter::InverseView | ViewParameter::InverseViewProjection => SpatialKind::ToWorld,
            ViewParameter::Projection
            | ViewParameter::InverseProjection
            | ViewParameter::ViewportSize
            | ViewParameter::InverseViewportSize
            | ViewParameter::NearPlane
            | ViewParameter::FarPlane
            | ViewParameter::InverseFarPlane
            | ViewParameter::ProjectionFlip
            | ViewParameter::ResolutionScale
            | ViewParameter::Perspective
            | ViewParameter::DepthRange
            | ViewParameter::DeviceDepthOffset
            | ViewParameter::DeviceDepthScale => SpatialKind::None,
        }
    }
}

impl ObjectMatrix {
    /// How the object's transform moves into a program frame: the object's space is converted as the world's is.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            ObjectMatrix::World => SpatialKind::WithinWorld,
            ObjectMatrix::WorldView | ObjectMatrix::WorldViewProjection => SpatialKind::FromWorld,
        }
    }
}

impl ObjectParameter {
    /// How the object's value moves into a program frame.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            ObjectParameter::World
            | ObjectParameter::InverseWorld
            | ObjectParameter::BonePalette
            | ObjectParameter::ClothCancelPalette => SpatialKind::WithinWorld,
            ObjectParameter::WorldViewProjection | ObjectParameter::WorldView => SpatialKind::FromWorld,
            ObjectParameter::ViewToObject => SpatialKind::ToWorld,
            ObjectParameter::PreviousFrame(m)
            | ObjectParameter::CameraRelative(m)
            | ObjectParameter::CameraRelativePreviousFrame(m) => m.spatial_kind(),
            ObjectParameter::ClipPlane => SpatialKind::Plane,
            ObjectParameter::CameraFadeInverseRange => SpatialKind::InverseLength,
            ObjectParameter::InstanceColour
            | ObjectParameter::TextureScroll(_)
            | ObjectParameter::EnvironmentData
            | ObjectParameter::MatrixBase
            | ObjectParameter::InstanceBase
            | ObjectParameter::ClothSimulation
            | ObjectParameter::TextureTransformRow(_)
            | ObjectParameter::PaletteCoordinate
            | ObjectParameter::CameraFadeEnabled
            | ObjectParameter::CameraFadeEnd
            | ObjectParameter::DitherAlphaEnabled
            | ObjectParameter::DitherEnabled
            | ObjectParameter::Fade
            | ObjectParameter::WindOff => SpatialKind::None,
        }
    }
}

impl FrameParameter {
    /// How the frame's value moves into a program frame.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            FrameParameter::LightDirection | FrameParameter::DirectionToLight => SpatialKind::Direction,
            FrameParameter::Time
            | FrameParameter::TimeOfDay
            | FrameParameter::PreExposure
            | FrameParameter::InversePreExposure
            | FrameParameter::Exposure
            | FrameParameter::InverseExposure
            | FrameParameter::ColourSlot
            | FrameParameter::LightColour
            | FrameParameter::LightSpecularColour
            | FrameParameter::DirectionalAmbientColour
            | FrameParameter::LightCount(_)
            | FrameParameter::LightTileSize
            | FrameParameter::LightTilesPerRow
            | FrameParameter::LocalShadowAtlasSize
            | FrameParameter::InverseLocalShadowAtlasSize => SpatialKind::None,
        }
    }
}

impl ItemParameter {
    /// How the item's value moves into a program frame.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            ItemParameter::LightRecord => SpatialKind::LightRecord,
        }
    }
}

impl ConstantRole {
    /// How the constant moves into a program frame. A material's own words, and a source's defaults, are the source's
    /// data, already in its programs' frame.
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            ConstantRole::View(v) => v.spatial_kind(),
            ConstantRole::Object(o) => o.spatial_kind(),
            ConstantRole::Frame(f) => f.spatial_kind(),
            ConstantRole::Item(i) => i.spatial_kind(),
            ConstantRole::Material(_)
            | ConstantRole::Default { .. }
            | ConstantRole::AlphaReference { .. }
            | ConstantRole::Unknown { .. } => SpatialKind::None,
        }
    }
}

impl LightValue {
    /// How the field of a light record moves into a program frame. The records are in view space, which keeps its own,
    /// but for the point a shadow is rendered from.
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            LightValue::ShadowCentre => SpatialKind::Point,
            LightValue::ViewPosition
            | LightValue::Range
            | LightValue::InverseRangeSquared
            | LightValue::InverseFalloffDistance
            | LightValue::TranslucencyStrength
            | LightValue::TranslucencyExponentScale
            | LightValue::Colour
            | LightValue::SpecularColour
            | LightValue::ViewToCone
            | LightValue::ViewBoundingSphere
            | LightValue::VerticalConeSphere { .. }
            | LightValue::ViewDepthMin
            | LightValue::ViewDepthMax
            | LightValue::Shadowed
            | LightValue::ShadowProjection { .. }
            | LightValue::ShadowFaces { .. }
            | LightValue::Constant { .. }
            | LightValue::Unknown { .. } => SpatialKind::None,
        }
    }
}

impl BufferTable {
    /// How the table's contents move into a program frame: its matrices from one space of the world's frame to
    /// another (a header word, and an instance record's indices, are none).
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            BufferTable::ObjectMatrices { .. }
            | BufferTable::HeaderedMatrices { .. }
            | BufferTable::SkinnedMatrices { .. } => SpatialKind::WithinWorld,
            BufferTable::Instances { .. } => SpatialKind::None,
        }
    }
}

impl BufferSource {
    /// How the buffer's contents move into a program frame. A graph's own buffers are written and read by the source's
    /// programs, in their frame.
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            BufferSource::Table(t) => t.spatial_kind(),
            BufferSource::Lights(_) => SpatialKind::LightRecord,
            BufferSource::LightLists(LightListPart::Lookup(_) | LightListPart::Indices(_))
            | BufferSource::Graph(_)
            | BufferSource::Unknown { .. } => SpatialKind::None,
        }
    }
}

impl FrameTexture {
    /// How the frame's image moves into a program frame.
    pub fn spatial_kind(self) -> SpatialKind {
        match self {
            FrameTexture::ReflectionProbe => SpatialKind::CubeByDirection,
            FrameTexture::LightProbeVolume => SpatialKind::VolumeByPosition,
            FrameTexture::DecalBuffer(1) => SpatialKind::ImageOfDirections,
            FrameTexture::DecalBuffer(_)
            | FrameTexture::SceneDepth
            | FrameTexture::SceneColour
            | FrameTexture::ShadowMap
            | FrameTexture::ScreenSpaceShadows
            | FrameTexture::AmbientOcclusion => SpatialKind::None,
        }
    }
}

impl TextureSource {
    /// How the bound image moves into a program frame. A source's own textures and a graph's own targets are in its
    /// programs' frame already.
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            TextureSource::Frame(t) => t.spatial_kind(),
            TextureSource::Texture { .. }
            | TextureSource::Default { .. }
            | TextureSource::Missing { .. }
            | TextureSource::Target { .. } => SpatialKind::None,
        }
    }
}

impl OutputRole {
    /// How what the program writes moves out of its frame: a world-space normal is the program frame's.
    pub fn spatial_kind(&self) -> SpatialKind {
        match self {
            OutputRole::Normal {
                space: NormalSpace::World,
                ..
            } => SpatialKind::Direction,
            OutputRole::Normal {
                space: NormalSpace::View | NormalSpace::Tangent,
                ..
            }
            | OutputRole::Albedo
            | OutputRole::Gloss { .. }
            | OutputRole::Roughness
            | OutputRole::Metallic
            | OutputRole::Occlusion { .. }
            | OutputRole::SpecularOcclusion
            | OutputRole::Translucency
            | OutputRole::Reflectance { .. }
            | OutputRole::SpecularIntensity { .. }
            | OutputRole::SpecularExponent { .. }
            | OutputRole::Fresnel
            | OutputRole::MotionVector
            | OutputRole::Emission { .. }
            | OutputRole::Colour
            | OutputRole::Coverage
            | OutputRole::Opacity
            | OutputRole::Unknown { .. } => SpatialKind::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Axis::{NegZ, PosX, PosY};

    const MIRROR: SourceFrame = SourceFrame::new([PosX, PosY, NegZ], 1.0);

    #[test]
    fn a_frame_is_stated_and_one_not_converted_says_why() {
        let w = World::new();
        assert_eq!(ProgramFrame::Source(MIRROR).frame(), Some(&MIRROR));
        assert_eq!(ProgramFrame::Source(MIRROR).verify(&w), Ok(()));
        let none = ProgramFrame::NotConverted { why: " ".into() };
        assert_eq!(none.frame(), None);
        assert!(none.verify(&w).unwrap_err().contains("why"));
        let json = ProgramFrame::Source(MIRROR).to_json().unwrap().unwrap();
        assert_eq!(json["source"]["axes"], json!(["PosX", "PosY", "NegZ"]));
    }

    /// The table on the roles whose kind decides where a drawn surface lands and faces: positions and their
    /// matrices, the light's direction, the camera, and what is left alone.
    #[test]
    fn each_role_states_how_it_moves() {
        assert_eq!(Attribute::Position.spatial_kind(), SpatialKind::Point);
        assert_eq!(Attribute::Point(3).spatial_kind(), SpatialKind::Point);
        assert_eq!(Attribute::Normal.spatial_kind(), SpatialKind::Direction);
        assert_eq!(Attribute::Tangent(0).spatial_kind(), SpatialKind::Tangent);
        assert_eq!(Attribute::Color(3).spatial_kind(), SpatialKind::None);
        let constant = |r: ConstantRole| r.spatial_kind();
        assert_eq!(
            constant(ConstantRole::View(ViewParameter::CameraPosition)),
            SpatialKind::Point
        );
        assert_eq!(
            constant(ConstantRole::View(ViewParameter::ViewProjection)),
            SpatialKind::FromWorld
        );
        assert_eq!(
            constant(ConstantRole::View(ViewParameter::InverseView)),
            SpatialKind::ToWorld
        );
        assert_eq!(
            constant(ConstantRole::View(ViewParameter::Projection)),
            SpatialKind::None
        );
        assert_eq!(
            constant(ConstantRole::Object(ObjectParameter::World)),
            SpatialKind::WithinWorld
        );
        assert_eq!(
            constant(ConstantRole::Object(ObjectParameter::CameraRelative(
                ObjectMatrix::WorldViewProjection
            ))),
            SpatialKind::FromWorld
        );
        assert_eq!(
            constant(ConstantRole::Object(ObjectParameter::ClipPlane)),
            SpatialKind::Plane
        );
        assert_eq!(
            constant(ConstantRole::Object(ObjectParameter::CameraFadeInverseRange)),
            SpatialKind::InverseLength
        );
        assert_eq!(
            constant(ConstantRole::Object(ObjectParameter::CameraFadeEnd)),
            SpatialKind::None
        );
        assert_eq!(
            constant(ConstantRole::Frame(FrameParameter::DirectionToLight)),
            SpatialKind::Direction
        );
        assert_eq!(
            constant(ConstantRole::Material(vec![0, 0, 0x3f80_0000])),
            SpatialKind::None
        );
        assert_eq!(
            InstanceValue::Axis(2).spatial_kind(),
            SpatialKind::WithinWorldColumn
        );
        assert_eq!(LightValue::ShadowCentre.spatial_kind(), SpatialKind::Point);
        assert_eq!(LightValue::ViewPosition.spatial_kind(), SpatialKind::None);
        assert_eq!(
            FrameTexture::ReflectionProbe.spatial_kind(),
            SpatialKind::CubeByDirection
        );
    }
}
