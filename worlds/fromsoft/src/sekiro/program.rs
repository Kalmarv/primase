//! The FromSoft preparation programs: the registered passes, in order, that lower what the area and character readers
//! produce for a program that draws or writes it, as pass texts a [`primase_core::pass::PassRegistry`] parses. Which
//! passes run and in what order is stated here once, and every program that lowers FromSoft data calls these builders;
//! every setting is the program's. Each builder takes a struct whose fields are the program's choices: what the read
//! took and which lowering it wants (which passes run), and each pass it runs as the program states it, as that pass's
//! own struct. Every field is required and the structs have no `Default`, so a program states each one.
//!
//! A builder writes each pass as the program stated it ([`pass_text`]) and never rewrites a stated value: where two of
//! the program's choices conflict, it refuses, naming both, and the program states the consistent value.
//!
//! [`AreaPreparation`] lowers an area read; [`CharacterPreparation`] lowers a character or a set of parts;
//! [`ClipPreparation`] lowers one loaded animation clip.
use fromsoft_formats::Game;
use primase_animation::AssembleCharacter;
use primase_animation::simulation_schedule::ConfigureSimulations;
use primase_core::Component;
use primase_core::SelectLod;
use primase_core::pass::pass_text;

use super::passes::bake_materials::BakeFromSoftMaterials;
use super::passes::collision::LowerFromSoftCollision;
use super::passes::draw_params::SampleFromSoftDrawParams;
use super::passes::effects::LowerFromSoftEffects;
use super::passes::environment::LowerFromSoftEnvironment;
use super::passes::lights::LowerFromSoftLights;
use super::passes::object_collision::{LowerFromSoftObjectCollision, LowerFromSoftPartCollision};
use super::passes::pbr::Layers;
use super::passes::records::LowerFromSoftMapRecords;
use super::passes::standalone::BindFromSoftAnimation;
use super::passes::weapons::HoldFromSoftWeapons;
use super::passes::{
    BakeFromSoftLayers, LowerFromSoftGeometry, LowerFromSoftMaterials, ResolveFromSoftShaders, Tiling,
};
use crate::materials::SkipClass;

/// The pass that lowers the area's render graph: the first of an area's preparation.
const RENDER_GRAPH: &str = "lower-fromsoft-render-graph";
/// The pass that lowers the sky an area read brought in to core's sky layer and anchors.
const SKY_PASS: &str = "lower-fromsoft-sky";
/// The release of the read's stored models and texture files ([`AreaPreparation::release_sources`]).
const RELEASE_SOURCES: &str = "release-fromsoft-sources";
/// The pass that keeps the form of a part a read was asked for: the first of a character's preparation.
const REQUESTED_FORM: &str = "select-fromsoft-requested-form";

/// The key a game's passes and sources are named by.
pub fn game_key(game: Game) -> &'static str {
    match game {
        Game::Sekiro => "sekiro",
        Game::EldenRing => "elden",
    }
}

/// What becomes of the layered source materials of an area or a character, and so whether the layer bake runs. The
/// program's `lower-fromsoft-materials` states the same choice (its `layers`); a builder refuses the two where they
/// differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Materials {
    /// Kept as source data: `lower-fromsoft-materials` with `layers=preserve` leaves their `fromsoft.material`, layered
    /// definition, bindings and per-primitive `fromsoft.layer_inputs` intact.
    Preserve,
    /// Lowered from their main-layer preview: `lower-fromsoft-materials` with `layers=preview`, a reported
    /// approximation that drops the layer inputs.
    Preview,
    /// Baked by the full public shader bake, `bake-fromsoft-layers` as stated, before the materials are lowered; the
    /// materials' lowering then states no `layers`, every layered material being baked.
    Bake(BakeFromSoftLayers),
}

/// The game of an area, with the choices only that game's lowering takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AreaGame {
    /// Sekiro.
    Sekiro {
        /// What becomes of the layered source materials.
        materials: Materials,
        /// `resolve-state` first, so the working world holds one concrete state at the axes' defaults while a reset
        /// snapshot taken before the preparation keeps the raw conditions.
        read_states: bool,
    },
    /// Elden Ring: its layered materials are what its `lower-fromsoft-materials` states (there is no layer bake in
    /// its lowering).
    EldenRing,
}

/// The package of Sekiro's irradiance a read was told to lower, chosen from source evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrradiancePackage {
    /// The area's full name (`m11_01_00_00`).
    pub area: String,
    /// The environment package index.
    pub package: u32,
}

/// An area's preparation: which passes run, from what the read took, and each pass as the program states it.
///
/// The order: the render graph (with the environment probes, where the read took them), the map's collision and the
/// objects' collision, Sekiro's distant backdrop dropped, the rigid skinning, the geometry, the level of detail, the
/// environment (after the draw-param sample, where there is one), the game's tone mapping, the shader resolution, the
/// layer bake (Sekiro, where the program bakes), the materials, the release of the read's stored models and texture
/// files (where the program releases them), the materials' surface bake, the lights, the sky (where the read brought it
/// in), the extras, the map records; then the irradiance, the effects and the entities' names. The
/// environment states the hour of the day (`core.time_of_day`) and runs before the first bake: a bake gives a program
/// that reads the hour the scene's, and refuses the program where the scene states none. The sky is lowered after the
/// lights, so after the environment, whose sun the moon stands towards, and after the materials, which it orders the
/// sky's draws over.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaPreparation {
    pub game: AreaGame,
    /// The read brought in the sky (`sky: include`): `lower-fromsoft-sky` lowers it after the lights. The geometry's
    /// `leave-out` must then keep the sky's meshes.
    pub sky: bool,
    /// The draw-param environment sampled at an hour of the game's clock before it is lowered. None: no pass samples,
    /// and the lowering uses its documented fallback hour, a counted approximation.
    pub draw_params: Option<SampleFromSoftDrawParams>,
    /// The read took environment probes (`select-fromsoft-environment`): place a far tile's groups, then lower and bind
    /// the probes after the render graph.
    pub environment_probes: bool,
    /// A weather snapshot was applied: `resolve-state` runs first.
    pub weather: bool,
    /// An irradiance package was selected: the area's state is assigned it first and the irradiance lowered last.
    pub irradiance_package: Option<IrradiancePackage>,
    /// The read took Sekiro's irradiance boxes and coefficient states: the irradiance is lowered last.
    pub irradiance_states: bool,
    /// The read took effects: they are lowered last, as stated.
    pub effects: Option<LowerFromSoftEffects>,
    /// The map's collision.
    pub collision: LowerFromSoftCollision,
    /// The objects' collision.
    pub object_collision: LowerFromSoftObjectCollision,
    /// The geometry. Its `uv-tiling` must be `keep`: the surface bake reads the authored UVs.
    pub geometry: LowerFromSoftGeometry,
    /// The level of detail each prototype keeps.
    pub lod: SelectLod,
    /// The draw-param environment.
    pub environment: LowerFromSoftEnvironment,
    /// Each material's game program bound, before the first pass that consumes the source materials: the bakes read
    /// the binding and the Game shader view draws it.
    pub resolve_shaders: ResolveFromSoftShaders,
    /// The materials. Its `layers` must be what the Sekiro `materials` choice states.
    pub lower_materials: LowerFromSoftMaterials,
    /// The surface bake of each resolved program, after the materials are lowered.
    pub material_bake: BakeFromSoftMaterials,
    /// The read's stored models and texture files released once the lowering has read them
    /// (`release-fromsoft-sources`, after the materials are lowered and before their surface bake, which reads
    /// neither): a program that writes what it lowered and lowers nothing again releases them; one that lowers again
    /// later (a viewer's state switch) keeps them.
    pub release_sources: bool,
    /// The lights.
    pub lights: LowerFromSoftLights,
    /// The map layout's regions and events.
    pub map_records: LowerFromSoftMapRecords,
}

impl AreaPreparation {
    /// The pass texts, in order; refused where two of the choices conflict.
    pub fn texts(&self) -> Result<Vec<String>, String> {
        if let AreaGame::Sekiro { materials, .. } = self.game {
            materials_agree(materials, &self.lower_materials)?;
        }
        keeps_the_authored_uvs(&self.geometry)?;
        if self.sky && self.geometry.leave_out.contains(&SkipClass::Sky) {
            return Err(
                "sky: the read brought in the sky, and lower-fromsoft-geometry's leave-out leaves out its meshes \
                 (sky); state a leave-out without sky"
                    .to_owned(),
            );
        }
        let environment = pass_text(&self.environment);
        let mut texts = vec![
            RENDER_GRAPH.to_owned(),
            pass_text(&self.collision),
            pass_text(&self.object_collision),
        ];
        if let AreaGame::Sekiro { .. } = self.game {
            texts.push("drop-fromsoft-far".to_owned());
        }
        texts.extend([
            "fromsoft-apply-rigid-skinning".to_owned(),
            pass_text(&self.geometry),
            pass_text(&self.lod),
        ]);
        if let Some(sample) = &self.draw_params {
            texts.push(pass_text(sample));
        }
        texts.push(environment);
        texts.push(
            match self.game {
                AreaGame::Sekiro { .. } => "lower-sekiro-reinhard",
                AreaGame::EldenRing => "lower-elden-ring-reinhard",
            }
            .to_owned(),
        );
        texts.push(pass_text(&self.resolve_shaders));
        if let AreaGame::Sekiro {
            materials: Materials::Bake(bake),
            ..
        } = self.game
        {
            texts.push(pass_text(&bake));
        }
        texts.push(pass_text(&self.lower_materials));
        if self.release_sources {
            texts.push(RELEASE_SOURCES.to_owned());
        }
        texts.extend([pass_text(&self.material_bake), pass_text(&self.lights)]);
        if self.sky {
            texts.push(SKY_PASS.to_owned());
        }
        texts.extend(["lower-fromsoft-extras".to_owned(), pass_text(&self.map_records)]);
        if self.environment_probes {
            texts.splice(
                1..1,
                [
                    "place-fromsoft-far-tile-environment".to_owned(),
                    "lower-fromsoft-environment-probes".to_owned(),
                    "bind-fromsoft-environment-probes".to_owned(),
                ],
            );
        }
        if let AreaGame::Sekiro {
            read_states: true, ..
        } = self.game
        {
            texts.insert(0, "resolve-state".to_owned());
        }
        if self.weather {
            texts.insert(0, "resolve-state".to_owned());
        }
        if let Some(package) = &self.irradiance_package {
            texts.insert(
                0,
                format!(
                    "resolve-state{{assign=fromsoft.irradiance.{}:package-{:02}}}",
                    package.area, package.package
                ),
            );
            texts.push("lower-fromsoft-irradiance".to_owned());
        }
        if self.irradiance_states {
            texts.push("lower-fromsoft-irradiance".to_owned());
        }
        if let Some(effects) = &self.effects {
            texts.push(pass_text(effects));
        }
        texts.push(format!("name-fromsoft-entities{{game={}}}", self.game_key()));
        Ok(texts)
    }

    fn game_key(&self) -> &'static str {
        match self.game {
            AreaGame::Sekiro { .. } => game_key(Game::Sekiro),
            AreaGame::EldenRing => game_key(Game::EldenRing),
        }
    }
}

/// `materials` and the materials' lowering state one choice: refused where `lower`'s `layers` is not the one
/// `materials` names.
fn materials_agree(materials: Materials, lower: &LowerFromSoftMaterials) -> Result<(), String> {
    let stated = match materials {
        Materials::Preserve => Some(Layers::Preserve),
        Materials::Preview => Some(Layers::Preview),
        Materials::Bake(_) => None,
    };
    if lower.layers() == stated {
        return Ok(());
    }
    let word = |layers: Option<Layers>| match layers {
        Some(Layers::Preserve) => "layers=preserve",
        Some(Layers::Preview) => "layers=preview",
        None => "no layers",
    };
    Err(format!(
        "materials {}: lower-fromsoft-materials states {}, and {} is that choice's",
        match materials {
            Materials::Preserve => "Preserve",
            Materials::Preview => "Preview",
            Materials::Bake(_) => "Bake",
        },
        word(lower.layers()),
        word(stated)
    ))
}

/// The surface bake every preparation runs reads the authored UVs, including their scale: refused where the
/// geometry multiplies them by the material's tiling.
fn keeps_the_authored_uvs(geometry: &LowerFromSoftGeometry) -> Result<(), String> {
    if geometry.uv_tiling == Tiling::Apply {
        return Err(
            "lower-fromsoft-geometry uv-tiling=apply: bake-fromsoft-materials, which the preparation runs, bakes over \
             the authored UVs; state uv-tiling=keep"
                .to_owned(),
        );
    }
    Ok(())
}

/// A character's or a set of parts' preparation: the geometry and material lowering they need, then the assembly of
/// its parts (each carries a skeleton of its own; one part is nothing to relate), then `place-assets` so the standalone
/// asset is drawn at the origin. No area collision, state, far, light or extra passes. Its texts do not depend on what
/// the read found: the form the read was asked for is kept by `select-fromsoft-requested-form`, the held weapons are
/// the assets the read marks `fromsoft.held_weapon`, and `hold-fromsoft-weapons` leaves a world with none as it is.
///
/// The order: the requested form kept, the geometry, the selected outfit (where it is dressed), the shader
/// resolution, the layer bake (where the program bakes), the materials and their surface bake, the part's collision,
/// the assembly, the animations' binding, the simulation schedule, the display masks and material events (where it is
/// dressed), the entities' names, `place-assets`, and the held weapons.
#[derive(Clone, Debug)]
pub struct CharacterPreparation {
    pub game: Game,
    /// What becomes of the layered source materials.
    pub materials: Materials,
    /// The animations bound onto the asset's skeleton.
    pub animations: Option<BindFromSoftAnimation>,
    /// The selected outfit's display masks, resolved after the geometry, and lowered with the material events.
    pub dressing: bool,
    /// The entities' names (`name-fromsoft-entities`).
    pub entity_names: bool,
    /// How held weapons are held, after the assets are placed. The assembly must leave out what the read marks held
    /// (`leave-out-marked=fromsoft.held_weapon`).
    pub held: Option<HoldFromSoftWeapons>,
    /// The geometry. Its `uv-tiling` must be `keep`: the surface bake reads the authored UVs.
    pub geometry: LowerFromSoftGeometry,
    /// Each material's game program bound before the materials are consumed.
    pub resolve_shaders: ResolveFromSoftShaders,
    /// The materials. Its `layers` must be what `materials` states.
    pub lower_materials: LowerFromSoftMaterials,
    /// The surface bake of each resolved program, after the materials are lowered.
    pub material_bake: BakeFromSoftMaterials,
    /// A part's own collision (a weapon's `_collidable.hkx`) as collision on the part.
    pub part_collision: LowerFromSoftPartCollision,
    /// How the parts are related into one character.
    pub assembly: AssembleCharacter,
    /// The schedule the character's simulations run at.
    pub simulations: ConfigureSimulations,
}

impl CharacterPreparation {
    /// The pass texts, in order; refused where two of the choices conflict.
    pub fn texts(&self) -> Result<Vec<String>, String> {
        materials_agree(self.materials, &self.lower_materials)?;
        keeps_the_authored_uvs(&self.geometry)?;
        let held = crate::sekiro::effect_data::HeldWeapon::NAME;
        if self.held.is_some() && self.assembly.leave_out_marked.as_deref() != Some(held) {
            return Err(format!(
                "hold-fromsoft-weapons holds the weapons the read marks {held}, but assemble-character leaves out {}: \
                 the assembly leaves out leave-out-marked={held}",
                self.assembly
                    .leave_out_marked
                    .as_deref()
                    .map_or_else(|| "nothing".to_owned(), |m| format!("what {m} marks"))
            ));
        }
        let key = game_key(self.game);
        let mut texts = vec![REQUESTED_FORM.to_owned(), pass_text(&self.geometry)];
        if self.dressing {
            texts.push("lower-fromsoft-selected-outfit".to_owned());
        }
        texts.push(pass_text(&self.resolve_shaders));
        if let Materials::Bake(bake) = self.materials {
            texts.push(pass_text(&bake));
        }
        texts.extend([
            pass_text(&self.lower_materials),
            pass_text(&self.material_bake),
            pass_text(&self.part_collision),
        ]);
        texts.push(pass_text(&self.assembly));
        if let Some(binding) = &self.animations {
            texts.push(pass_text(binding));
        }
        texts.push(pass_text(&self.simulations));
        if self.dressing {
            texts.push("lower-fromsoft-display-masks".to_owned());
            texts.push(format!("lower-fromsoft-material-events{{game={key}}}"));
        }
        if self.entity_names {
            texts.push(format!("name-fromsoft-entities{{game={key}}}"));
        }
        texts.push("place-assets".to_owned());
        if let Some(hold) = &self.held {
            texts.push(pass_text(hold));
        }
        Ok(texts)
    }
}

/// A loaded animation clip's preparation: the animations bound onto the skeleton of the asset, the clip's events, the
/// display masks and material events, the entities' names.
#[derive(Clone, Debug)]
pub struct ClipPreparation {
    pub game: Game,
    pub binding: BindFromSoftAnimation,
    /// The effects the clip's events start, lowered as stated after the events.
    pub effects: Option<LowerFromSoftEffects>,
}

impl ClipPreparation {
    /// The pass texts, in order.
    pub fn texts(&self) -> Vec<String> {
        let key = game_key(self.game);
        let mut texts = vec![
            pass_text(&self.binding),
            format!("lower-fromsoft-clip-events{{game={key}}}"),
        ];
        if let Some(effects) = &self.effects {
            texts.push(pass_text(effects));
        }
        texts.extend([
            "lower-fromsoft-display-masks".to_owned(),
            format!("lower-fromsoft-material-events{{game={key}}}"),
            format!("name-fromsoft-entities{{game={key}}}"),
        ]);
        texts
    }
}

/// The passes that choose a character's outfit, after its geometry is lowered: the display masks its meshes are marked
/// with become state axes, the NPC param row `npc` sets them (each traced in the pass's log) and `resolve-state` drops
/// the primitives whose mask is off. Where a model's weapons and equipment are mask groups (c3000's `#00#` to `#12#`),
/// this is what chooses which of them are drawn. With no row (`None`) every mask is hidden (`npc=none`: the unmarked
/// meshes, the body, are drawn): a statement that no row was chosen, not a default row.
#[cfg(test)]
fn outfit_passes(chr: &str, npc: Option<i32>) -> [String; 3] {
    let chosen = match npc {
        Some(npc) => format!("npc={npc}"),
        None => "npc=none".to_owned(),
    };
    [
        "raise-fromsoft-display-masks".to_owned(),
        format!("select-fromsoft-outfit{{chr={chr},{chosen}}}"),
        "resolve-state".to_owned(),
    ]
}

#[cfg(test)]
mod tests;
