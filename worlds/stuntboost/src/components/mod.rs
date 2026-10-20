//! The `stuntboost.*` dialect: what a STUNTBOOST level states, as stored. Positions are in the core frame in metres, at
//! the scale the program read the level with ([Frame](crate#frame)).
use std::collections::BTreeMap;

use primase_core::{Component, Entity, JsonError};
use serde::Serialize;

fn to_json<T: Serialize>(v: &T) -> Result<Option<serde_json::Value>, JsonError> {
    primase_core::json::to_value(v).map(Some)
}

/// What a level part is, as the game tells them apart: the part with a spawn (`=Spawn...`) is the level, the one with
/// a skybox (`=Skybox...`) the sky, any other the room around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuntboostRole {
    Level,
    Room,
    Skybox,
}

/// A level part: the level's name (`Skatepark` for `Level_Skatepark`), the part's number and its role. On its own
/// entity; every node of the part names it ([`StuntboostNode::part`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostPart {
    pub level: String,
    pub index: u32,
    pub role: StuntboostRole,
    /// The file it was read from.
    pub file: String,
}

impl Component for StuntboostPart {
    const NAME: &'static str = "stuntboost.part";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// How a node takes part in the lighting bake (the game's `NameBakeShadows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuntboostBake {
    /// Visible in the bake and baked to (the default; `CastAndReceive`).
    Baked,
    /// `*`: visible in the bake but not baked to (`Cast`).
    NotBakedTo,
    /// `°`, or an invisible node: invisible in the bake (`None`).
    HiddenInBake,
}

/// How a node collides in game (the game's `Aggregate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuntboostCollision {
    None,
    /// `<`: solid.
    Solid,
    /// `#`: a trigger (passed through).
    Trigger,
}

/// One call of a node's code: `Spawn(start)`, `Grind()`, `BoomboxButton` (no parentheses: `args` none).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostCall {
    pub name: String,
    pub args: Option<Vec<String>>,
}

/// A node name parsed by the game's rules ([`crate::read::names`]): `raw` as stored, the name and its type characters,
/// the code as the game keeps it (from the `=` on), its class (`None`: `Entity()`) and the calls after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostName {
    pub raw: String,
    pub name: String,
    /// `_`: invisible in game and in the bake.
    pub invisible: bool,
    pub bake: StuntboostBake,
    pub collision: StuntboostCollision,
    /// The code from its `=` on, whole (empty when there is none).
    pub code: String,
    pub class: Option<StuntboostCall>,
    pub methods: Vec<StuntboostCall>,
    /// Blender's `.NNN` duplicate suffix: what the game drops after a name without code, or what ends a code.
    pub duplicate: Option<u32>,
}

/// How the game draws a node (its level draw rules, `DrawLevel`), from its name.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuntboostDraw {
    /// Drawn.
    Visible,
    /// `_`: never drawn (collision-only `_<`, invisible helpers).
    Invisible,
    /// `BackfaceShadowCaster`: drawn only into the static shadows, never visibly (every room's ceiling).
    ShadowOnly,
    /// A name containing `Transparent`: drawn sorted back to front, with the `Transparent(alpha)` call's alpha when it
    /// states one.
    Transparent { alpha: Option<f32> },
}

impl StuntboostName {
    /// The class name, `Entity` when none is written.
    pub fn class_name(&self) -> &str {
        self.class.as_ref().map_or("Entity", |c| c.name.as_str())
    }

    /// Every call, the class first.
    pub fn calls(&self) -> impl Iterator<Item = &StuntboostCall> {
        self.class.iter().chain(&self.methods)
    }

    /// How the game draws the node, by its rules in this order: invisible (`_`), shadow only (a
    /// `BackfaceShadowCaster` call), transparent (the name contains `Transparent`), else visible.
    pub fn draw(&self) -> StuntboostDraw {
        if self.invisible {
            return StuntboostDraw::Invisible;
        }
        if self.calls().any(|c| c.name == "BackfaceShadowCaster") {
            return StuntboostDraw::ShadowOnly;
        }
        if self.raw.contains("Transparent") {
            let alpha = (self.calls().find(|c| c.name == "Transparent"))
                .and_then(|c| c.args.as_ref()?.first()?.parse().ok());
            return StuntboostDraw::Transparent { alpha };
        }
        StuntboostDraw::Visible
    }
}

/// A node of a level part: its part's entity, its glTF node index and its name parsed, with its `extras` as stored
/// (`sbe_properties`, `sbe_parent_collection`, ...). On the node's entity (a placement when it has a mesh).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostNode {
    pub part: Entity,
    pub node: u32,
    pub name: StuntboostName,
    pub extras: Option<serde_json::Value>,
}

impl Component for StuntboostNode {
    const NAME: &'static str = "stuntboost.node";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.part);
    }
}

/// A texture a material binds: the texture entity and its glTF sampler's filters as stored (`None`: not given).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostTextureRef {
    pub texture: Entity,
    pub mag_filter: Option<u32>,
    pub min_filter: Option<u32>,
    pub texcoord: u32,
}

/// A glTF material as stored: its name, the PBR factors and textures, whether it is double-sided and its alpha mode.
/// A bake material's `KHR_materials_specular` colour texture is not a specular colour: it is the bake's direct-light
/// mask, [`StuntboostDirectLight`] on the same entity. A bake material's base
/// colour texture is the baked atlas: lighting baked in. Collision materials (`C_*`) are physics materials
/// ([`StuntboostPhysics`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostMaterial {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_color_texture: Option<StuntboostTextureRef>,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub double_sided: bool,
    /// `OPAQUE`, `MASK` or `BLEND`, and `MASK`'s cutoff.
    pub alpha_mode: String,
    pub alpha_cutoff: Option<f32>,
    /// What else the material states, by key, as stored (extensions, extras).
    pub other: BTreeMap<String, serde_json::Value>,
}

impl Component for StuntboostMaterial {
    const NAME: &'static str = "stuntboost.material";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.base_color_texture.iter().map(|t| t.texture));
    }
}

/// A physics material's name parsed: `C_<behaviour>[_<substance>][_no_grind]`, e.g. `C_Default_Wood`,
/// `C_Ramp_MetalHollow`, `C_Kill`, `C_RailBoost`, `C_Default_PlasticHard_no_grind` (a Blender `.NNN` suffix dropped).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostPhysics {
    /// The name without its `.NNN` suffix.
    pub name: String,
    /// `Default`, `Ramp`, `RampV`, `RampVExtension`, `Kill`, `RailBoost`, `RailNoBoost`, or another word as stored.
    pub behaviour: String,
    /// `Wood`, `Metal`, `MetalHollow`, `Cardboard`, `Fabric`, `PlasticHard`, `PlasticSoft`, ...; `None`: none written.
    pub substance: Option<String>,
    pub no_grind: bool,
}

impl StuntboostPhysics {
    /// A material name parsed as a physics material; `None` when it is not one (no `C_` prefix).
    pub fn parse(material: &str) -> Option<StuntboostPhysics> {
        let name = match material.rsplit_once('.') {
            Some((n, d)) if d.len() == 3 && d.bytes().all(|b| b.is_ascii_digit()) => n,
            _ => material,
        };
        let rest = name.strip_prefix("C_")?;
        let (rest, no_grind) = match rest.strip_suffix("_no_grind") {
            Some(r) => (r, true),
            None => (rest, false),
        };
        let (behaviour, substance) = match rest.split_once('_') {
            Some((b, s)) => (b, Some(s.to_owned())),
            None => (rest, None),
        };
        Some(StuntboostPhysics {
            name: name.to_owned(),
            behaviour: behaviour.to_owned(),
            substance,
            no_grind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The physics materials the levels use parse into behaviour, substance and the grind flag.
    #[test]
    fn physics_material_names_parse() {
        let p = StuntboostPhysics::parse("C_Default_PlasticHard_no_grind.002").unwrap();
        assert_eq!(
            (
                p.name.as_str(),
                p.behaviour.as_str(),
                p.substance.as_deref(),
                p.no_grind
            ),
            (
                "C_Default_PlasticHard_no_grind",
                "Default",
                Some("PlasticHard"),
                true
            )
        );
        let p = StuntboostPhysics::parse("C_RampVExtension_Cardboard").unwrap();
        assert_eq!(
            (p.behaviour.as_str(), p.substance.as_deref()),
            ("RampVExtension", Some("Cardboard"))
        );
        let p = StuntboostPhysics::parse("C_Kill").unwrap();
        assert_eq!(
            (p.behaviour.as_str(), p.substance, p.no_grind),
            ("Kill", None, false)
        );
        assert!(StuntboostPhysics::parse("bake_mat_0").is_none());
    }
}

/// An application-specific vertex attribute of a primitive as stored (`_COLOR_SHIFT`: the outline offset the game's
/// inverted-hull outlines push each vertex by): its component type, whether normalized, its element type, its count
/// (the primitive's vertex count) and its elements packed, little-endian.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostAttribute {
    pub component: String,
    pub normalized: bool,
    pub element: String,
    pub count: usize,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl From<&primase_glb::AccessorData> for StuntboostAttribute {
    fn from(a: &primase_glb::AccessorData) -> StuntboostAttribute {
        StuntboostAttribute {
            component: format!("{:?}", a.component),
            normalized: a.normalized,
            element: format!("{:?}", a.element),
            count: a.count,
            bytes: a.bytes.clone(),
        }
    }
}

/// A prototype's application-specific vertex attributes, by primitive (in the mesh's order), by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostAttributes(pub Vec<BTreeMap<String, StuntboostAttribute>>);

impl Component for StuntboostAttributes {
    const NAME: &'static str = "stuntboost.attributes";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A bake group's direct-light mask (`Bake<N>_DIRECT_Denoised`, stored as the bake material's `KHR_materials_specular`
/// colour texture, which the game binds as its `DiffuseTex`): the light that reached each texel straight from the sun,
/// without bounces. A baked level (one with a `SetBakedLighting` node) draws unlit:
/// `colour = atlas * (1 + k * ShadowColor) * lerp(AmbientLightInShadow, 1, 1 - k)` with
/// `k = clamp((mask.r - 0.07) * 1.075, 0, 1) * dynamic shadow`, so the mask only gates the dynamic shadows (the
/// board's) to directly lit texels (the defaults: `ShadowColor` 0, `AmbientLightInShadow` 0.5). No N.L, no specular.
/// On the material's entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostDirectLight {
    pub texture: StuntboostTextureRef,
}

impl Component for StuntboostDirectLight {
    const NAME: &'static str = "stuntboost.direct_light";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.texture.texture);
    }
}

/// One part of a node's collision in the game's own collision model: its triangles (world corners) and, for a solid
/// mesh, each triangle's grippable-edge bits (which edges the board's trucks can lock onto) and ramp (an index into the
/// part file's ramps, -1 none); both empty for a trigger.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostGameCollisionPart {
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub grippable: Vec<u8>,
    pub ramps: Vec<i32>,
}

/// A node's collision as the game's pipeline derived it (`Level_<L>_<N>Collision.xnb`, its collision processor run
/// over the part's glTF): its aggregate (solid or trigger) and its parts. The source of truth for what the game
/// collides with and how its board grips; `lower-stuntboost-collision` derives core collision from the glTF instead.
/// On the node's entity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostGameCollision {
    pub aggregate: StuntboostCollision,
    pub parts: Vec<StuntboostGameCollisionPart>,
}

impl Component for StuntboostGameCollision {
    const NAME: &'static str = "stuntboost.game_collision";
    /// Counts, not the triangles.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "aggregate": self.aggregate,
            "triangles": self.parts.iter().map(|p| p.triangles.len()).collect::<Vec<_>>(),
        })))
    }
}

/// A grind ledge's kind, as the game names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuntboostLedgeKind {
    /// A grindable edge of the geometry (`core.curve` kind `ledge`).
    Edge,
    /// A rail (`core.curve` kind `rail`), without and with the boost it gives.
    RailNoBoost,
    RailBoost,
}

/// A grind ledge as the game's pipeline derived it: its kind, its two corners (indices into its part's
/// [`StuntboostLedgeGraph`]), its frame (normal, tangent, and the binormal the game derives from them), the angles
/// between which the board may grind it (radians), its length and its recursive lengths in each direction (metres),
/// its group, and its lerp cut-offs as stored. Its entity carries the segment as `core.curve`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostLedge {
    pub part: Entity,
    pub kind: StuntboostLedgeKind,
    pub corners: [u32; 2],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub binormal: [f32; 3],
    pub min_angle: f32,
    pub max_angle: f32,
    pub length: f32,
    pub length_recursive: [f32; 2],
    pub group: i32,
    pub lerp_cut_off: [f32; 2],
}

impl Component for StuntboostLedge {
    const NAME: &'static str = "stuntboost.ledge";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.part);
    }
}

/// A ledge corner: its position, the corners near it (indices) and the ledges leaving it (entity, forward).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostLedgeCorner {
    pub position: [f32; 3],
    pub near: Vec<u32>,
    pub ledges: Vec<(Entity, bool)>,
}

/// A part's grind ledge graph as the game's pipeline derived it (how ledges join at corners), and its collision grid's
/// cell sizes for solids and triggers (metres). On the part's entity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StuntboostLedgeGraph {
    pub solids_cell: [f32; 3],
    pub triggers_cell: [f32; 3],
    pub corners: Vec<StuntboostLedgeCorner>,
}

impl Component for StuntboostLedgeGraph {
    const NAME: &'static str = "stuntboost.ledge_graph";
    /// Counts, not the corners.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "solids_cell": self.solids_cell,
            "triggers_cell": self.triggers_cell,
            "corners": self.corners.len(),
        })))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.corners.iter().flat_map(|c| c.ledges.iter().map(|(e, _)| *e)));
    }
}

/// A part's shortcuts as the game's pipeline stored them (`Level_<L>_<N>Cuts.xnb`): per key (a level's id), its
/// integers as stored (`None`: a null array). On the part's entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StuntboostShortcuts(pub BTreeMap<u32, Option<Vec<i32>>>);

impl Component for StuntboostShortcuts {
    const NAME: &'static str = "stuntboost.shortcuts";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}
