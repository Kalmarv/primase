//! Elden Ring's map wind as the game builds it for cloth, read from the install: the force field each wind effect region
//! (layout region type 6) spawns from the wind force action (FXR action 10300) of the effect it names, placed as the game
//! places it. What the field does to a cloth is the cloth crate's; this reads where it stands and what it holds.
//!
//! What the game does, read from its code (`tests/map_wind.rs` holds these values to the bytes of a force field the
//! running game built):
//! - Loading a layout converts a region's rotation from the file's degrees to radians, `f32(deg) * f32(pi / 180)`. For a
//!   rotation about y alone the rows of its matrix are `(cos y, 0, -sin y)`, `(0, 1, 0)` and `(sin y, 0, cos y)` (the C
//!   runtime's single-precision `sinf` and `cosf`), and the field's wind blows along the third row.
//! - The effect is spawned at the region's matrix with the world block's offset added to the translation: the field's
//!   transform. The offset is the game's floating origin, which moves while the game runs and is not in the install, so
//!   it is an input stated for a moment ([`BlockOffset`]).
//! - The field's area is the box of the region the type data names; its centre is the area's x and z and
//!   `(position.y + height / 2) + block.y`, and the field's origin has that y.
//! - The field's parameters are the action's fields as stored, under [`WindForce`]'s names.
//!
//! Observed, not read from the code: which fields act on a cloth. In the one capture checked, the field acting on the
//! player's cloth was the one wind effect region of `m10_00_00_00`'s twelve whose area box (from its position up to its
//! height) held the cloth; [`sources_at`] keeps the regions whose box holds a position.
//!
//! Refused by name ([`Refusal`]): the field's origin in x and z (the capture's is 2^-19 off the box centre's in x, from
//! arithmetic between the area and the field's creation not reproduced yet), a rotation about x or z (the game's product
//! order is not reproduced), an area that is another region (whose rotation the field takes is not observed) or not a
//! box, and an effect no binder holds or with other than one wind force action.
use super::effects::{EffectLibrary, binders_for_area};
use fromsoft_formats::fxr::actions::WindForce;
use fromsoft_formats::msb::{MapLayout, Region, RegionShape};
use fromsoft_formats::{Archive, Game, Result, dcx::Decoder};

/// `f32(pi / 180)`, the factor the game converts a layout's degrees by.
pub const RADIANS_PER_DEGREE: f32 = f32::from_bits(0x3c8e_fa35);

/// The world block's offset at a moment of the running game (its floating origin), with where the value comes from. The
/// game's frame is the map's moved by it.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockOffset {
    pub offset: [f32; 3],
    /// Where the value was read or how it was derived (a capture and its step).
    pub provenance: String,
}

/// A wind effect region's force field, as the game places it.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The wind effect region and the region bounding it.
    pub region: String,
    pub area: String,
    /// The effect the region spawns and its wind force action.
    pub effect: i32,
    pub force: WindForce,
    /// The rows of the region's rotation as the game builds them; the wind blows along `rows[2]`.
    pub rows: [[f32; 3]; 3],
    /// The region's position in the map's frame: the field's translation less the block offset.
    pub position: [f32; 3],
    /// The area box's centre in the map's frame: the area's x and z, its y plus half its height.
    pub area_centre: [f32; 3],
    /// The area box's width, height and depth, along the rows.
    pub area_size: [f32; 3],
}

/// Why a wind effect region gives no [`Source`], or why a value of one is not given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The type data names no region (-1).
    NoArea,
    /// The area is another region (`area`): whose rotation the field takes is not observed.
    AreaIsAnotherRegion { area: String },
    /// The area is not a box (`shape` names what it is).
    AreaNotBox { shape: &'static str },
    /// The region rotates about x or z: the game's product order is not reproduced.
    RotationNotAboutY,
    /// No binder searched holds the effect.
    NoEffectFile,
    /// The effect's file does not read (the reason).
    EffectUnread { reason: String },
    /// The effect holds `count` wind force actions, not one.
    WindForceCount { count: usize },
    /// The field's origin in x and z: the arithmetic between the area and the field's creation is not reproduced (2^-19
    /// off the box centre's x in the one capture checked).
    OriginCreationArithmetic,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoArea => write!(f, "wind field source: the region names no area"),
            Refusal::AreaIsAnotherRegion { area } => {
                write!(
                    f,
                    "wind field source: the area is another region ({area}), not observed"
                )
            }
            Refusal::AreaNotBox { shape } => write!(f, "wind field source: the area is a {shape}, not a box"),
            Refusal::RotationNotAboutY => write!(
                f,
                "wind field direction: a rotation about x or z is not reproduced"
            ),
            Refusal::NoEffectFile => write!(f, "wind field source: no binder holds the effect"),
            Refusal::EffectUnread { reason } => {
                write!(f, "wind field source: the effect does not read: {reason}")
            }
            Refusal::WindForceCount { count } => {
                write!(
                    f,
                    "wind field source: the effect holds {count} wind force actions, not one"
                )
            }
            Refusal::OriginCreationArithmetic => {
                write!(
                    f,
                    "wind field origin x/z: creation arithmetic not traced (2^-19 at x on er-wind2)"
                )
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// A wind effect region's [`Source`], or its name and why it gives none.
pub type Resolved = std::result::Result<Source, (String, Refusal)>;

impl Source {
    /// The direction the wind blows: the rotation's third row.
    pub fn direction(&self) -> [f32; 3] {
        self.rows[2]
    }

    /// The field's translation in the game's frame at `block`: the region's position plus the offset.
    pub fn translation(&self, block: &BlockOffset) -> [f32; 3] {
        std::array::from_fn(|i| self.position[i] + block.offset[i])
    }

    /// The area box's centre in the game's frame at `block`; its y is the field's origin's.
    pub fn area_centre_at(&self, block: &BlockOffset) -> [f32; 3] {
        std::array::from_fn(|i| self.area_centre[i] + block.offset[i])
    }

    /// The field's origin in the game's frame: refused ([`Refusal::OriginCreationArithmetic`]).
    pub fn origin(&self, _block: &BlockOffset) -> std::result::Result<[f32; 3], Refusal> {
        Err(Refusal::OriginCreationArithmetic)
    }

    /// Whether the area box holds `point` (the map's frame): along each row, within half the box's size of its centre.
    pub fn area_holds(&self, point: [f64; 3]) -> bool {
        let d: [f64; 3] = std::array::from_fn(|i| point[i] - f64::from(self.area_centre[i]));
        self.rows.iter().zip(self.area_size).all(|(row, size)| {
            let along: f64 = row.iter().zip(&d).map(|(&r, &x)| f64::from(r) * x).sum();
            along.abs() <= f64::from(size) / 2.
        })
    }
}

/// The rows of a region's rotation as the game builds them, for a rotation about y alone.
pub fn rows(rotation_degrees: [f32; 3]) -> std::result::Result<[[f32; 3]; 3], Refusal> {
    let [x, y, z] = rotation_degrees;
    if x != 0. || z != 0. {
        return Err(Refusal::RotationNotAboutY);
    }
    let radians = y * RADIANS_PER_DEGREE;
    let (s, c) = (radians.sin(), radians.cos());
    Ok([[c, 0., -s], [0., 1., 0.], [s, 0., c]])
}

fn shape_name(shape: &RegionShape) -> &'static str {
    match shape {
        RegionShape::None => "region with no shape",
        RegionShape::Point => "point",
        RegionShape::Circle { .. } => "circle",
        RegionShape::Sphere { .. } => "sphere",
        RegionShape::Cylinder { .. } => "cylinder",
        RegionShape::Rectangle { .. } => "rectangle",
        RegionShape::Box { .. } => "box",
        RegionShape::Composite { .. } => "composite",
        RegionShape::Unknown { .. } => "shape of an unnamed code",
    }
}

/// One wind effect region resolved, or why it is not.
fn resolve(
    library: &mut EffectLibrary<'_>,
    regions: &[Region],
    region: &Region,
    effect: i32,
    area: Option<usize>,
) -> Result<std::result::Result<Source, Refusal>> {
    let Some(area) = area.and_then(|i| regions.get(i)) else {
        return Ok(Err(Refusal::NoArea));
    };
    if area.index != region.index {
        return Ok(Err(Refusal::AreaIsAnotherRegion {
            area: area.name.clone(),
        }));
    }
    let RegionShape::Box { width, depth, height } = area.shape else {
        return Ok(Err(Refusal::AreaNotBox {
            shape: shape_name(&area.shape),
        }));
    };
    let rows = match rows(region.rotation_degrees) {
        Ok(rows) => rows,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let file = match library.effect(effect)? {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(Err(Refusal::NoEffectFile)),
        Err(reason) => return Ok(Err(Refusal::EffectUnread { reason })),
    };
    let mut forces = file
        .actions()
        .into_iter()
        .filter(|a| a.id == WindForce::ID)
        .map(|a| WindForce::read(a, Game::EldenRing))
        .collect::<Result<Vec<_>>>()?;
    if forces.len() != 1 {
        return Ok(Err(Refusal::WindForceCount { count: forces.len() }));
    }
    let [x, y, z] = area.position;
    Ok(Ok(Source {
        region: region.name.clone(),
        area: area.name.clone(),
        effect,
        force: forces.remove(0),
        rows,
        position: region.position,
        area_centre: [x, y + height * 0.5, z],
        area_size: [width, height, depth],
    }))
}

/// Every wind effect region of Elden Ring map `map` (`m10_00_00_00`) in layout order, resolved or refused by name. The
/// effects are looked for in the map's binders, then the common ones ([`binders_for_area`]).
pub fn sources(archive: &Archive, decoder: &Decoder<'_>, map: &str) -> Result<Vec<Resolved>> {
    if archive.game() != Game::EldenRing {
        return Err(crate::invalid(
            "map wind",
            0,
            format!(
                "{:?}: only Elden Ring's wind effect regions are read",
                archive.game()
            ),
        ));
    }
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{map}.msb.dcx"), decoder)?)?;
    let regions = layout.map_regions()?;
    let mut library = EffectLibrary::new(
        archive,
        decoder,
        Game::EldenRing,
        binders_for_area(Game::EldenRing, map),
    );
    let mut out = Vec::new();
    for region in &regions {
        let Some(wind) = layout.wind_sfx_region(region)? else {
            continue;
        };
        out.push(
            resolve(&mut library, &regions, region, wind.effect_id, wind.wind_area)?
                .map_err(|refusal| (region.name.clone(), refusal)),
        );
    }
    Ok(out)
}

/// The wind effect regions of `map` whose area box holds `position` (the map's frame), resolved, and every region
/// refused, since whether its box holds the position is not known.
pub fn sources_at(
    archive: &Archive,
    decoder: &Decoder<'_>,
    map: &str,
    position: [f64; 3],
) -> Result<Vec<Resolved>> {
    Ok(sources(archive, decoder, map)?
        .into_iter()
        .filter(|r| r.as_ref().is_err() || r.as_ref().is_ok_and(|s| s.area_holds(position)))
        .collect())
}
