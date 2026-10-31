//! The FromSoft preparation programs: the registered passes, in order, that lower what the area and character readers
//! produce for a program that draws or writes it, as pass texts a [`primase_core::pass::PassRegistry`] parses. The pass
//! list is part of the program, so it is stated here once and every program that lowers FromSoft data calls these
//! builders. Each builder takes a struct whose fields are the program's choices; every field is required and the
//! structs have no `Default`, so a program states each one.
//!
//! [`AreaPreparation`] lowers an area read; [`CharacterPreparation`] lowers a character or a set of parts;
//! [`ClipPreparation`] lowers one loaded animation clip.
use fromsoft_formats::Game;

use super::passes::bake_materials::Cache;

/// The public FromSoft lowering for an area, in the order the primase example runs it: Sekiro's source records become
/// the core components a viewer draws (geometry, materials, collision, lights, extras). Every choice is the example's
/// faithful one, but for the object collision's convex bodies, given their rounded surface (the core swept by the
/// Havok radius) within 0.01 m (`convex=rounded:0.01`) rather than the sharp intersection of their offset planes; a
/// SpeedTree mesh's normal and tangent are the members its program reads (`speedtree-frame=program`). The environment
/// is lowered before the materials ([`ENVIRONMENT_PASS`]): it states the hour of the day, which the bakes read.
const AREA_PREP: &[&str] = &[
    "lower-fromsoft-render-graph",
    "lower-fromsoft-collision{reach=50}",
    "lower-fromsoft-object-collision{resolution=high,convex=rounded:0.01,motion=fixed,convex-surface=render,reach=50}",
    "drop-fromsoft-far",
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
    ENVIRONMENT_PASS,
    "lower-sekiro-reinhard",
    "bake-fromsoft-layers{max-size=512,sample-time=0}",
    MATERIALS_PASS,
    "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,min-range=0.5}",
    "lower-fromsoft-extras",
    MAP_RECORDS_PASS,
];

/// Where an area recipe lowers the source materials: the builder puts the form the program's choices state in its place
/// (the layer options and the water colour).
const MATERIALS_PASS: &str = "lower-fromsoft-materials";

/// The map layout's regions (sounds, effects, wind, environment-map points, weather overrides ...) as markers named by
/// their type and its events as named records, none left out: the display lists every one, and the markers' kinds tell
/// them apart.
const MAP_RECORDS_PASS: &str = "lower-fromsoft-map-records{leave-out=}";

/// The draw-param environment's lowering (`fromsoft.draw_param` to the sun and ambient of `core.environment`), run
/// where the area was read with a draw-param file and skipped where it was not. The display shows the sun's direction
/// and colour as its key light; the intensity a display shows the key light at is its own setting, so the calibration
/// states only the stored colours' scale (1 lux for a colour of 1). It states the hour of the day too
/// (`core.time_of_day`: the draw-param sample's, else the reader's default hour), so it runs before the first bake: a
/// bake gives a program that reads the hour the scene's, and refuses the program where the scene states none.
const ENVIRONMENT_PASS: &str = "lower-fromsoft-environment{lux-per-unit=1}";

/// Elden Ring's area lowering: the same passes, with its main collision (hit filter 8, as its layouts' collision parts
/// use it) and its own HitMtrl classes (the passes read the game's table from the read: Sand is 3 here, Dirt 5): the
/// water class 21 (its param row "Water") is the water soup, and the assets' collision leaves it out as Sekiro's
/// leaves out its water classes. The class 0 ("None") that Sekiro's 29 would correspond to is not left out: whether
/// ER's untagged collision is invisible blockers or ordinary ground is a census of the install to answer
/// (`tests/hit_class_census.rs`), so its triangles stay ground, counted by class. Its assets' collision is
/// the high-resolution file where the game stores one, else the low one every collidable asset has, no motion type
/// left out (its stored motion types are not mapped yet), convex bodies rounded within 0.01 m as Sekiro's; SpeedTree's
/// billboard cards left out (`billboard`: their program turns them to the camera, the trees' own meshes are drawn) and
/// a SpeedTree mesh's normal and tangent the members its program reads. `surface_bakes` adds the resolved program's
/// static surface bake while retaining its material source and binding for the Game shader view. Its environment is
/// lowered before the materials as Sekiro's is ([`ENVIRONMENT_PASS`]): its sky programs read the hour.
const ELDEN_AREA_PREP: &[&str] = &[
    "lower-fromsoft-render-graph",
    "lower-fromsoft-collision{reach=50}",
    "lower-fromsoft-object-collision{resolution=high-else-low,convex=rounded:0.01,motion=all,convex-surface=render,reach=50}",
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
    ENVIRONMENT_PASS,
    "lower-elden-ring-reinhard",
    MATERIALS_PASS,
    "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,min-range=0.5}",
    "lower-fromsoft-extras",
    MAP_RECORDS_PASS,
];

/// Binds each layered material's G-buffer program and constants, textures and outputs as roles (`core.shader_binding`).
const RESOLVE_SHADERS: &str = "resolve-fromsoft-shaders{unbound-textures=preview}";
/// A part's collision (a weapon's `_collidable.hkx`) as collision on the part, for the Collision view: convex bodies as
/// their rounded surface within 1 cm (the area's choice), every stored motion type lowered at its stored pose.
const PART_COLLISION: &str = "lower-fromsoft-part-collision{convex=rounded:0.01,motion=all}";
/// The pass that lowers the sky an area read brought in to core's sky layer and anchors.
const SKY_PASS: &str = "lower-fromsoft-sky";

/// How several parts are related into one character: the rule `assemble-character` takes (`by-name` correspondence of
/// joints, each part's skins moved onto the assembled skeleton at the assembled rest, rest disagreements up to a
/// millimetre taken, one a part does not weight reported, not refused). A weighted joint that disagrees by more keeps
/// the part's own inverse bind (`weighted-rests=report`): the game poses a part through its own inverse binds, so the
/// face of Ashina's Memory costume (`_m_9120_l` parts: its Head and the joints under it stand 4.7e-3 from the other parts',
/// 0.27 degrees) is drawn about a millimetre off its stored rest, as the game draws it; in the Wolf's default parts the
/// body's Head stands 4.675e-3 from the face's. Every such joint and the displacement are in the preparation
/// report as approximations.
pub const ASSEMBLE_PARTS: &str = "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled}";

/// The simulation schedule a character's simulations run at: a 30 Hz step, 30 steps of pre-roll, at most 8 steps per
/// frame.
const CONFIGURE_SIMULATIONS: &str =
    "configure-simulations{step-seconds=0.03333333333333333,pre-roll=30,max-steps-per-frame=8}";

/// The key a game's passes and sources are named by.
pub fn game_key(game: Game) -> &'static str {
    match game {
        Game::Sekiro => "sekiro",
        Game::EldenRing => "elden",
    }
}

/// How an area's or a character's source materials are lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Materials {
    /// Layered source materials are kept as source data (`layers=preserve`): their `fromsoft.material`, layered
    /// definition, bindings and per-primitive `fromsoft.layer_inputs` intact.
    Preserve,
    /// Primase's main-layer preview instead (`layers=preview`), a reported approximation that drops the layer inputs.
    Preview,
    /// The full public shader bake (`bake-fromsoft-layers`).
    Bake,
}

/// The game of an area, with the choices only that game's lowering takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaGame {
    /// Sekiro.
    Sekiro {
        /// How the source materials are lowered.
        materials: Materials,
        /// Resolve each material's game program first (`resolve-fromsoft-shaders`) even without the layer bake, for
        /// the Game shader view.
        game_shaders: bool,
        /// `resolve-state` first, so the working world holds one concrete state at the axes' defaults while a reset
        /// snapshot taken before the preparation keeps the raw conditions.
        read_states: bool,
    },
    /// Elden Ring: layers preserved, programs resolved for the Game shader view.
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

/// An area's preparation: the lowering and the program's choices about it.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaPreparation {
    pub game: AreaGame,
    /// The read brought in the sky (`sky: include`): its geometry is kept and `lower-fromsoft-sky` lowers it after the
    /// lights, so after the environment, whose sun the moon stands towards.
    pub sky: bool,
    /// The colour of water where it has no texture: sRGB bytes and a linear alpha byte.
    pub water_colour: [u8; 4],
    /// The hour of the game's clock (0 to 24) at which each time-of-day field of the draw-param environment is
    /// sampled (`sample-fromsoft-draw-params`). None: no pass samples, and the lowering uses its documented fallback
    /// hour, a counted approximation.
    pub hour: Option<f32>,
    /// The read took environment probes (`select-fromsoft-environment`): lower and bind them after the render graph.
    pub environment_probes: bool,
    /// A weather snapshot was applied: `resolve-state` runs first.
    pub weather: bool,
    /// An irradiance package was selected: the area's state is assigned it first and the irradiance lowered last.
    pub irradiance_package: Option<IrradiancePackage>,
    /// The read took Sekiro's irradiance boxes and coefficient states: the irradiance is lowered last.
    pub irradiance_states: bool,
    /// The read took effects: `lower-fromsoft-effects` runs last.
    pub effects: bool,
    /// Whether the material bake looks each material up in, and stores it to, the caller's memo store.
    pub bake_cache: Cache,
}

impl AreaPreparation {
    /// The pass texts, in order.
    pub fn texts(&self) -> Vec<String> {
        let mut texts = match self.game {
            AreaGame::EldenRing => {
                let water = water_option(self.water_colour);
                let recipe = ELDEN_AREA_PREP
                    .iter()
                    .map(|text| match *text {
                        MATERIALS_PASS => {
                            format!("lower-fromsoft-materials{{layers=preserve,water-colour={water}}}")
                        }
                        text => text.to_owned(),
                    })
                    .collect();
                surface_bakes(with_sky(recipe, self.sky), self.bake_cache)
            }
            AreaGame::Sekiro {
                materials,
                game_shaders,
                read_states,
            } => sekiro_area(
                materials,
                game_shaders,
                read_states,
                self.sky,
                self.water_colour,
                self.bake_cache,
            ),
        };
        if self.environment_probes {
            let after = texts
                .iter()
                .position(|text| text == "lower-fromsoft-render-graph")
                .map_or(texts.len(), |index| index + 1);
            texts.splice(
                after..after,
                [
                    "lower-fromsoft-environment-probes".into(),
                    "bind-fromsoft-environment-probes".into(),
                ],
            );
        }
        if self.weather {
            texts.insert(0, "resolve-state".into());
        }
        if let Some(hour) = self.hour
            && let Some(index) = texts.iter().position(|text| text == ENVIRONMENT_PASS)
        {
            texts.insert(index, format!("sample-fromsoft-draw-params{{hour={hour}}}"));
        }
        if let Some(package) = &self.irradiance_package {
            texts.insert(
                0,
                format!(
                    "resolve-state{{assign=fromsoft.irradiance.{}:package-{:02}}}",
                    package.area, package.package
                ),
            );
            texts.push("lower-fromsoft-irradiance".into());
        }
        if self.irradiance_states {
            texts.push("lower-fromsoft-irradiance".into());
        }
        if self.effects {
            texts.push("lower-fromsoft-effects{candela-per-power=1}".to_owned());
        }
        texts.push(format!("name-fromsoft-entities{{game={}}}", self.game_key()));
        texts
    }

    fn game_key(&self) -> &'static str {
        match self.game {
            AreaGame::Sekiro { .. } => game_key(Game::Sekiro),
            AreaGame::EldenRing => game_key(Game::EldenRing),
        }
    }
}

/// Sekiro's area lowering ([`AREA_PREP`]) with the materials the choices state. With the layer bake the full public
/// shader bake runs. Otherwise layered source materials are kept as source data (`layers=preserve`) or lowered by
/// primase's main-layer preview (`layers=preview`).
fn sekiro_area(
    materials: Materials,
    game_shaders: bool,
    read_states: bool,
    sky: bool,
    water_colour: [u8; 4],
    cache: Cache,
) -> Vec<String> {
    let bake = materials == Materials::Bake;
    let water = water_option(water_colour);
    let mut texts: Vec<String> = AREA_PREP
        .iter()
        .flat_map(|text| {
            // The binding the bake reads and the Game shader view draws from, resolved once before the first pass that
            // consumes the source materials.
            let first_consumer = if bake {
                "bake-fromsoft-layers"
            } else {
                "lower-fromsoft-materials"
            };
            let mut out = Vec::new();
            if (bake || game_shaders) && text.starts_with(first_consumer) {
                out.push(RESOLVE_SHADERS.to_owned());
            }
            out.extend(if !bake && text.starts_with("bake-fromsoft-layers") {
                None
            } else if *text == MATERIALS_PASS {
                Some(match materials {
                    Materials::Bake => format!("lower-fromsoft-materials{{water-colour={water}}}"),
                    Materials::Preview => {
                        format!("lower-fromsoft-materials{{layers=preview,water-colour={water}}}")
                    }
                    Materials::Preserve => {
                        format!("lower-fromsoft-materials{{layers=preserve,water-colour={water}}}")
                    }
                })
            } else {
                Some((*text).into())
            });
            out
        })
        .collect();
    if read_states {
        texts.insert(0, "resolve-state".into());
    }
    surface_bakes(with_sky(texts, sky), cache)
}

/// Both surface views consume the program: Shaded bakes its surface channels, Game shader draws it directly.
/// Keep one resolution before either bake; the texture-space bake reads the authored UVs, including their scale.
fn surface_bakes(texts: Vec<String>, cache: Cache) -> Vec<String> {
    let resolved = texts.iter().any(|s| s.starts_with("resolve-fromsoft-shaders"));
    let mut inserted = resolved;
    let mut result = Vec::new();
    for text in texts {
        if !inserted
            && (text.starts_with("lower-fromsoft-materials") || text.starts_with("bake-fromsoft-layers"))
        {
            result.push(RESOLVE_SHADERS.to_owned());
            inserted = true;
        }
        let lower = text.starts_with("lower-fromsoft-materials");
        result.push(text.replace("uv-tiling=apply", "uv-tiling=keep"));
        if lower {
            result.push(format!(
                "bake-fromsoft-materials{{max-size=2048,sample-time=0,cache={}}}",
                cache_text(cache)
            ));
        }
    }
    result
}

fn cache_text(cache: Cache) -> &'static str {
    match cache {
        Cache::On => "on",
        Cache::Off => "off",
    }
}

fn water_option([r, g, b, a]: [u8; 4]) -> String {
    format!("{r},{g},{b},{a}")
}

/// `texts`, an area's lowering, for a read that brought in the sky (`sky`): the sky's meshes kept by the geometry
/// lowering (what else it leaves out stays out) and [`SKY_PASS`] after the lights, so after the environment, whose sun
/// the moon stands towards (and after the materials, which it orders the sky's draws over); else `texts` as they are.
fn with_sky(texts: Vec<String>, sky: bool) -> Vec<String> {
    if !sky {
        return texts;
    }
    texts
        .into_iter()
        .flat_map(|text| {
            if text.starts_with("lower-fromsoft-geometry") {
                vec![text.replace("leave-out=sky+", "leave-out=")]
            } else if text.starts_with("lower-fromsoft-lights") {
                vec![text, SKY_PASS.into()]
            } else {
                vec![text]
            }
        })
        .collect()
}

/// The animation a clip binds: the Havok animations lowered to clips on the model's skeleton, the skeletons made to
/// correspond by name and rest, the clips mapped by that. Each choice is the program's: `absent` (what a component a
/// track does not store stands for), `rate` (clip keys per source frame), `clips` (which animations: `all`, or names
/// joined by `;`) and `root_motion` (what becomes of the extracted motion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipBinding {
    /// The character whose animation binders are read (`c1010`; the player's is `c0000`).
    pub of: String,
    /// The model skeleton the clips are mapped onto, named after the asset's reader or its assembly.
    pub skeleton: String,
    /// How closely the Havok skeleton's rests must agree with the model's to correspond.
    pub tolerance: String,
    pub absent: String,
    pub rate: u32,
    pub clips: String,
    pub root_motion: String,
}

impl ClipBinding {
    /// The pass that makes the animations clips on the model's skeleton: lower the Havok animations to clips, make the
    /// Havok skeleton correspond to the model's by name and rest, then map the clips by that.
    pub fn pass(&self) -> String {
        format!(
            "bind-fromsoft-animation{{from={} havok skeleton,to={},tolerance={},empty=model-space,absent={},rate={},root-motion={},clips={}}}",
            self.of, self.skeleton, self.tolerance, self.absent, self.rate, self.root_motion, self.clips
        )
    }
}

/// Held weapons: the sources of their models' assets (the body's assembly leaves them out) and the explicitly stated
/// grip they are held in after the assets are placed (`hold-fromsoft-weapons`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldWeapons {
    pub held: Vec<String>,
    pub grip: String,
}

/// A character's or a set of parts' preparation: the geometry and material lowering they need, then (for several
/// parts, which each carry a skeleton of their own) `assemble-character`, then `place-assets` so the standalone asset
/// is drawn at the origin. No area collision, state, far, light or extra passes.
#[derive(Clone, Debug)]
pub struct CharacterPreparation {
    pub game: Game,
    /// How the source materials are lowered.
    pub materials: Materials,
    /// Resolve each material's game program first, as an area's preparation does, so the Game shader view draws the
    /// character through its programs.
    pub game_shaders: bool,
    /// The colour of water where it has no texture: sRGB bytes and a linear alpha byte.
    pub water_colour: [u8; 4],
    /// How many parts the asset assembles: more than one are related into one character.
    pub parts: usize,
    /// The names `select-assets` keeps (one form of a part and the animations' asset): none keeps everything.
    pub select_assets: Vec<String>,
    /// The animations bound onto the asset's skeleton.
    pub animations: Option<ClipBinding>,
    /// The selected outfit's display masks, resolved after the geometry, and lowered with the material events.
    pub dressing: bool,
    /// The entities' names (`name-fromsoft-entities`).
    pub entity_names: bool,
    /// The weapons held and their grip.
    pub held: Option<HeldWeapons>,
    /// Whether the material bake looks each material up in, and stores it to, the caller's memo store.
    pub bake_cache: Cache,
}

impl CharacterPreparation {
    /// The pass texts, in order.
    pub fn texts(&self) -> Vec<String> {
        let mut texts = character_lowering(
            self.materials,
            self.game_shaders,
            self.water_colour,
            self.bake_cache,
        );
        if self.parts > 1 {
            texts.push(ASSEMBLE_PARTS.to_owned());
        }
        if let Some(binding) = &self.animations {
            texts.push(binding.pass());
        }
        texts.push(CONFIGURE_SIMULATIONS.to_owned());
        texts.push("place-assets".to_owned());
        if self.dressing {
            texts.insert(1, "lower-fromsoft-selected-outfit".into());
        }
        if !self.select_assets.is_empty() {
            texts.insert(
                0,
                format!("select-assets{{names={}}}", self.select_assets.join(";")),
            );
        }
        let key = game_key(self.game);
        let mut before_placing = Vec::new();
        if self.dressing {
            before_placing.push("lower-fromsoft-display-masks".to_owned());
            before_placing.push(format!("lower-fromsoft-material-events{{game={key}}}"));
        }
        if self.entity_names {
            before_placing.push(format!("name-fromsoft-entities{{game={key}}}"));
        }
        let final_place = texts.len() - 1;
        texts.splice(final_place..final_place, before_placing);
        if let Some(held) = &self.held {
            held_weapon_passes(&mut texts, &held.held, &held.grip);
        }
        texts
    }
}

/// With held weapons (the sources of their models' assets, `held`): the body's assembly leaves them out, and after the
/// assets are placed `hold-fromsoft-weapons` puts each where the explicitly stated grip holds it.
fn held_weapon_passes(texts: &mut Vec<String>, held: &[String], grip: &str) {
    if held.is_empty() {
        return;
    }
    for text in texts.iter_mut().filter(|t| t.as_str() == ASSEMBLE_PARTS) {
        *text = format!(
            "{},leave-out={}}}",
            ASSEMBLE_PARTS.trim_end_matches('}'),
            held.join("+")
        );
    }
    let after = texts
        .iter()
        .position(|t| t == "place-assets")
        .map_or(texts.len(), |i| i + 1);
    texts.insert(after, format!("hold-fromsoft-weapons{{grip={grip}}}"));
}

/// The geometry and material lowering a character needs before anything is related to its skeleton or placed. With
/// `game_shaders` (or the layer bake, which reads the binding) `resolve-fromsoft-shaders` binds each material's G-buffer
/// program first, as the area's preparation does, so the Game shader view draws the character through its programs.
fn character_lowering(
    materials: Materials,
    game_shaders: bool,
    water_colour: [u8; 4],
    cache: Cache,
) -> Vec<String> {
    let water = water_option(water_colour);
    // Characters are skinned: preserve their model-space bind pose (`pose=stored`), source winding, degenerates and
    // UVs, unlike the area's rigid policy. `place-assets` then places the standalone asset at the origin; the
    // reader's skeleton and rig-instance bindings are what the mesh draws through.
    let mut texts = vec![
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}".to_owned(),
    ];
    if materials == Materials::Bake || game_shaders {
        texts.push(RESOLVE_SHADERS.to_owned());
    }
    match materials {
        Materials::Bake => {
            texts.push("bake-fromsoft-layers{max-size=512,sample-time=0}".to_owned());
            texts.push(format!("lower-fromsoft-materials{{water-colour={water}}}"));
        }
        Materials::Preview => texts.push(format!(
            "lower-fromsoft-materials{{layers=preview,water-colour={water}}}"
        )),
        Materials::Preserve => texts.push(format!(
            "lower-fromsoft-materials{{layers=preserve,water-colour={water}}}"
        )),
    }
    texts.push(PART_COLLISION.to_owned());
    surface_bakes(texts, cache)
}

/// A loaded animation clip's preparation: the animations bound onto the skeleton of the asset, the clip's events, the
/// display masks and material events, the entities' names.
#[derive(Clone, Debug)]
pub struct ClipPreparation {
    pub game: Game,
    pub binding: ClipBinding,
    /// The effects the clip's events start are lowered too.
    pub effects: bool,
}

impl ClipPreparation {
    /// The pass texts, in order.
    pub fn texts(&self) -> Vec<String> {
        let key = game_key(self.game);
        let mut texts = vec![self.binding.pass()];
        texts.extend([
            format!("lower-fromsoft-clip-events{{game={key}}}"),
            "lower-fromsoft-display-masks".into(),
            format!("lower-fromsoft-material-events{{game={key}}}"),
            format!("name-fromsoft-entities{{game={key}}}"),
        ]);
        if self.effects {
            texts.insert(
                texts.len() - 3,
                "lower-fromsoft-effects{candela-per-power=1}".into(),
            );
        }
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
