//! `lower-anvil-grass{rect=x0,y0,x1,y1,range=METRES,offset=METRES,sample=corner|centre,seed=N,
//! density=per-sample|per-square-metre,tiling=rule|one,scale=rule|one,height-scale=rule|none,tilt=none|rule,
//! normal=terrain|up,max-plants=N}`: grass plants, placed by **a stand-in for the engine's run-time scatter**.
//!
//! What a forge stores of Riders Republic's grass is its models (46 `GrassVisual` Meshes, [`AnvilGrassModel`]), a
//! library of groups of rules ([`AnvilGrass`]) and, per terrain node, the group of the library to use at each 0.5 m
//! ground sample ([`AnvilTerrainGroups`], slot 5). **No plant is stored**: the engine creates the instances at run
//! time, camera-dependent, and the code that does is in no forge resource and not in the memory image the Riders
//! Republic notes hold (docs/grass.md §0, §5). This pass therefore *generates* content: it is allowed only as the named
//! approximation it is, every choice below a required option, nothing defaulted, and it reports the plants it made
//! (`grass plants placed by a stand-in for the engine's run-time scatter`) beside what it did not do. **The faithful
//! rule is the engine's own scatter**, which the Riders Republic memory dumps (no game launch: the game has an
//! anti-cheat) can recover: a later reverse-engineering item, after which this pass's options give way to the rule.
//!
//! What is read, and what is chosen:
//!
//! - **Where and which** (read; the picker is the engine's reading, docs/vegetation.md §3.4): a sample whose group is
//!   not 254 (none) and whose weight (slot 4, halved) is not 0 takes that group's rules; each rule names a model by
//!   its number ([`crate::read::grass::StoredGrassRule::model`], the setup's position). A group or model that doesn't
//!   exist is counted and left out (the library's rules name models 46 to 50 of its 46).
//! - **How many** (chosen: `density`, `tiling`): the notes cannot say whether `Density` is a per-sample probability, a
//!   count per square metre or scaled by the tiling. `density=per-sample` draws, for each candidate, one plant with
//!   probability `Density`; `density=per-square-metre` draws `Density x (a sample's area in m^2)` plants per candidate
//!   (its whole part, and one more with the fraction as probability). `tiling=rule` takes `4^TilingLogCount`
//!   candidates a sample for a rule (the notes' recipe: 2 on four rules), `tiling=one` one.
//! - **Where in the sample, the yaw, the seed** (chosen: `seed`): each plant lies at a uniform point of the half-sample
//!   square about its sample point (`sample`, as `lower-anvil-terrain`'s: the same value puts the plants on its
//!   ground), its yaw uniform, its random numbers a splitmix64 stream keyed by `seed`, the sample's world index, the
//!   group and the rule's position: the same options give the same plants, and neighbouring samples are independent.
//! - **Height and slope** (read where stated): the ground's height at the plant (bilinear between the node's height
//!   samples: `range` x 2^(level - 28) per value + `offset`, as the ground's), the plant's up following the ground's
//!   normal by the model's `TerrainNormalFollowerRatio` (1 on all 46) where `normal=terrain`, else the source's up.
//! - **Size** (chosen: `scale`, `height-scale`, `tilt`): `scale=rule` draws a uniform scale between the rule's `ScaleB`
//!   and `ScaleA` where its `ScalingEnabled` is set (the notes' reading by name and value, not run), `one` none;
//!   `height-scale=rule` the same between `HeightScaleB` and `HeightScaleA` along the up axis alone where
//!   `HeightScalingEnabled` is set; `tilt=rule` leans the up by an angle (radians) uniform up to the rule's
//!   `TiltFactor` in a uniform direction (a stand-in: the field's meaning is a lead), `none` not.
//! - **A plant** is an [`Instance`] of its model's prototype (the Mesh the model placement holds; its LOD ranges are
//!   the mesh lowering's to choose) with a [`Transform`]; the model placements and the setup are consumed.
//!
//! `max-plants` is a ceiling the pass refuses (an error, nothing truncated) when the options would make more.
//!
//! Left out, counted or reported: the engine's draw distance (`GrassVisibilityDistance` 250 m and the models' LOD
//! switch distances: the plants of `rect` all exist), its per-instance dither fade and wind and bend constants (the
//! `MAD_Clutters_Grass` shader's, engine-bound), the camera-dependent selection, and the 82 samples-per-metre-squared
//! limits of the instance buffers.

use std::collections::BTreeMap;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, Name, Transform, World};

use super::{CellPoint, OFFSET, RECT, SAMPLE, TERRAIN_CORE, TERRAIN_TILE};
use crate::components::{
    AnvilGrass, AnvilGrassModel, AnvilTerrain, AnvilTerrainGroups, GroupTile, TerrainTile,
};

/// How `Density` is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    /// A probability per candidate and sample.
    PerSample,
    /// A count per square metre.
    PerSquareMetre,
}

impl Choice for Density {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("per-sample", Density::PerSample),
        ("per-square-metre", Density::PerSquareMetre),
    ];
}

/// Options `tiling` and `scale`: the rule's, or one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleOrOne {
    Rule,
    One,
}

impl Choice for RuleOrOne {
    const VALUES: &'static [(&'static str, Self)] = &[("rule", RuleOrOne::Rule), ("one", RuleOrOne::One)];
}

/// Options `height-scale` and `tilt`: the rule's, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleOrNone {
    Rule,
    None,
}

impl Choice for RuleOrNone {
    const VALUES: &'static [(&'static str, Self)] = &[("rule", RuleOrNone::Rule), ("none", RuleOrNone::None)];
}

/// Option `normal`: a plant's up the ground's normal, or up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlantUp {
    Terrain,
    Up,
}

impl Choice for PlantUp {
    const VALUES: &'static [(&'static str, Self)] = &[("terrain", PlantUp::Terrain), ("up", PlantUp::Up)];
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilGrass {
    /// The rectangle (x0, y0, x1, y1, source units) whose plants are made.
    pub rect: [f64; 4],
    /// The terrain's height range in metres and offset: as `lower-anvil-terrain`'s.
    pub range: f64,
    pub offset: f64,
    pub sample: CellPoint,
    pub seed: u64,
    pub density: Density,
    /// Whether a rule's `TilingLogCount` multiplies its candidates (`4^count`).
    pub tiling: bool,
    /// Whether a rule's scale range applies.
    pub scale: bool,
    pub height_scale: bool,
    pub tilt: bool,
    /// Whether a plant's up follows the ground's normal.
    pub normal_terrain: bool,
    pub max_plants: u64,
}

impl LowerAnvilGrass {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<LowerAnvilGrass, String> {
        let rect: Vec<f64> = g
            .required("rect")
            .split(',')
            .map(|v| v.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| "rect is four numbers x0,y0,x1,y1".to_owned())?;
        let rect: [f64; 4] = rect
            .try_into()
            .map_err(|_| "rect is four numbers x0,y0,x1,y1".to_owned())?;
        if !(rect.iter().all(|v| v.is_finite()) && rect[0] < rect[2] && rect[1] < rect[3]) {
            return Err(format!("rect {rect:?}: finite, x0 < x1 and y0 < y1"));
        }
        let finite = |k: &str| -> Result<f64, String> {
            let v: f64 = g.required(k).parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("{k} {v}: a finite number"))
            }
        };
        let range = finite("range")?;
        let offset = finite("offset")?;
        let seed: u64 = g
            .required("seed")
            .parse()
            .map_err(|_| "seed is a whole number".to_owned())?;
        let max_plants: u64 = g
            .required("max-plants")
            .parse()
            .map_err(|_| "max-plants is a whole number".to_owned())?;
        if max_plants == 0 {
            return Err("max-plants is at least 1".to_owned());
        }
        Ok(LowerAnvilGrass {
            rect,
            range,
            offset,
            sample: g.choice("sample"),
            seed,
            density: g.choice("density"),
            tiling: g.choice::<RuleOrOne>("tiling") == RuleOrOne::Rule,
            scale: g.choice::<RuleOrOne>("scale") == RuleOrOne::Rule,
            height_scale: g.choice::<RuleOrNone>("height-scale") == RuleOrNone::Rule,
            tilt: g.choice::<RuleOrNone>("tilt") == RuleOrNone::Rule,
            normal_terrain: g.choice::<PlantUp>("normal") == PlantUp::Terrain,
            max_plants,
        })
    }
}

/// `lower-anvil-grass`' declaration ([`primase_core::pass::PassRegistry::add`]): the plants are not stored, so every
/// choice of the stand-in scatter is the program's.
pub const LOWER_ANVIL_GRASS: PassSpec = PassSpec {
    name: "lower-anvil-grass",
    options: &[
        RECT,
        OptionSpec::required(
            "range",
            "1",
            "the terrain's height range, finite: as lower-anvil-terrain's",
        ),
        OFFSET,
        SAMPLE,
        OptionSpec::required("seed", "0", "the scatter's seed, a whole number"),
        OptionSpec::choice::<Density>(
            "density",
            "a rule's Density as a probability per candidate and sample, or a count per square metre",
        ),
        OptionSpec::choice::<RuleOrOne>(
            "tiling",
            "a rule's TilingLogCount multiplies its candidates (4^count), or one",
        ),
        OptionSpec::choice::<RuleOrOne>("scale", "a rule's scale range applies, or one"),
        OptionSpec::choice::<RuleOrNone>("height-scale", "a rule's height scale applies, or none"),
        OptionSpec::choice::<RuleOrNone>("tilt", "a rule's tilt applies, or none"),
        OptionSpec::choice::<PlantUp>("normal", "a plant's up follows the ground's normal, or is up"),
        OptionSpec::required("max-plants", "1000000", "the most plants made, at least 1"),
    ],
    make: |g| Ok(Box::new(LowerAnvilGrass::from_given(g)?)),
};

/// `tiling`'s and `scale`'s word for whether the rule's applies.
fn rule_or_one(rule: bool) -> &'static str {
    if rule { RuleOrOne::Rule } else { RuleOrOne::One }.text()
}

/// `height-scale`'s and `tilt`'s word for whether the rule's applies.
fn rule_or_none(rule: bool) -> &'static str {
    if rule { RuleOrNone::Rule } else { RuleOrNone::None }.text()
}

/// splitmix64's output function.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A splitmix64 stream.
struct Rng(u64);

impl Rng {
    /// The stream of one rule at one sample: keyed by the seed, the sample's world indices (`gx`, `gy`), the group and the
    /// rule's position in it.
    fn new(seed: u64, gx: u32, gy: u32, group: u8, rule: usize) -> Rng {
        let s = mix(seed);
        let s = mix(s ^ (u64::from(gx) << 32 | u64::from(gy)));
        Rng(mix(s ^ (u64::from(group) << 8 | rule as u64)))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform between `a` and `b`.
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
}

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        a.map(|x| x / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    a.map(|x| x * k)
}

/// A node's ground, the heights its 132 x 132 samples hold (the core from index 2) in the terrain's source units.
struct Ground<'t> {
    tile: &'t TerrainTile,
    unit: f64,
    offset: f64,
    spacing: f64,
}

impl Ground<'_> {
    /// The height at local sample (`i`, `j`): row `i` along y, column `j` along x, each from -2 to 129 (the tile's 132
    /// samples from its index 0: the two before a node's core are its neighbours', the core's 128 follow, and the two
    /// after). A plant lies within half a sample of a core sample, so its four samples are -1 to 128: an index outside
    /// the tile is an error naming it, never an edge sample taken for it.
    fn height(&self, i: i64, j: i64) -> Result<f64, String> {
        let at = |v: i64| {
            v.checked_add(TerrainTile::CORE_AT as i64)
                .and_then(|v| usize::try_from(v).ok())
                .filter(|&v| v < TERRAIN_TILE)
                .ok_or_else(|| {
                    format!("ground sample ({i}, {j}) lies outside the node's tile of {TERRAIN_TILE} samples")
                })
        };
        let (r, c) = (at(i)?, at(j)?);
        Ok(self.unit * f64::from(self.tile.heights[r * TERRAIN_TILE + c]) + self.offset)
    }
    /// The height and the unit normal (source frame, z up) at fractional local sample (`fi`, `fj`): bilinear between the
    /// four samples about it, the normal that surface's.
    fn at(&self, fi: f64, fj: f64) -> Result<(f64, V3), String> {
        let (i0, j0) = (fi.floor(), fj.floor());
        let (ty, tx) = (fi - i0, fj - j0);
        let (i0, j0) = (i0 as i64, j0 as i64);
        let (h00, h01) = (self.height(i0, j0)?, self.height(i0, j0 + 1)?);
        let (h10, h11) = (self.height(i0 + 1, j0)?, self.height(i0 + 1, j0 + 1)?);
        let z = (1.0 - ty) * ((1.0 - tx) * h00 + tx * h01) + ty * ((1.0 - tx) * h10 + tx * h11);
        let dzdx = ((1.0 - ty) * (h01 - h00) + ty * (h11 - h10)) / self.spacing;
        let dzdy = ((1.0 - tx) * (h10 - h00) + tx * (h11 - h01)) / self.spacing;
        Ok((z, norm([-dzdx, -dzdy, 1.0])))
    }
}

/// A plant's transform in the source frame: yaw `yaw` about the up `up` (the source's z leaned toward it), scaled by
/// `s` and by `hs` along the up.
fn plant(yaw: f64, up: V3, s: f64, hs: f64, at: V3) -> Transform {
    let reference = [yaw.cos(), yaw.sin(), 0.0];
    let x = norm(add(reference, mul(up, -dot(reference, up))));
    let y = cross(up, x);
    let col = |v: V3, k: f64| [(v[0] * k) as f32, (v[1] * k) as f32, (v[2] * k) as f32];
    Transform {
        cols: [col(x, s), col(y, s), col(up, s * hs), col(at, 1.0)],
    }
}

impl Pass for LowerAnvilGrass {
    fn name(&self) -> &'static str {
        "lower-anvil-grass"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("range".to_owned(), self.range.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            ("sample".to_owned(), self.sample.text().to_owned()),
            ("seed".to_owned(), self.seed.to_string()),
            ("density".to_owned(), self.density.text().to_owned()),
            ("tiling".to_owned(), rule_or_one(self.tiling).to_owned()),
            ("scale".to_owned(), rule_or_one(self.scale).to_owned()),
            (
                "height-scale".to_owned(),
                rule_or_none(self.height_scale).to_owned(),
            ),
            ("tilt".to_owned(), rule_or_none(self.tilt).to_owned()),
            (
                "normal".to_owned(),
                if self.normal_terrain {
                    PlantUp::Terrain
                } else {
                    PlantUp::Up
                }
                .text()
                .to_owned(),
            ),
            ("max-plants".to_owned(), self.max_plants.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[
            AnvilGrass::NAME,
            AnvilGrassModel::NAME,
            AnvilTerrain::NAME,
            AnvilTerrainGroups::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[Instance::NAME, Transform::NAME, Name::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilGrass::NAME, AnvilGrassModel::NAME, AnvilTerrainGroups::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "grass placed by a stand-in for the engine's run-time scatter: the plants are not stored in any forge, so their \
         number, places, yaw, scale and lean are the options' choices over the stored models, rules and ground \
         rasters; the faithful rule is the engine's own scatter, recoverable from the memory dumps"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let setups: Vec<Entity> = w.iter::<AnvilGrass>().map(|(e, _)| e).collect();
        let [setup] = setups[..] else {
            return Err(format!(
                "one grass setup is expected, the world holds {}",
                setups.len()
            ));
        };
        let library = w
            .get::<AnvilGrass>(setup)
            .map(|g| g.library.clone())
            .ok_or("the setup is gone")?;
        // the models' prototypes and names, by number; their placements are the pass's to consume
        let mut models: BTreeMap<u8, (Entity, Option<Name>)> = BTreeMap::new();
        let mut placements = Vec::new();
        for (e, m) in w.iter::<AnvilGrassModel>() {
            placements.push(e);
            let Some(prototype) = w.get::<Instance>(e).map(|i| i.prototype) else {
                return Err(format!(
                    "grass model {} has no placement to take its prototype from",
                    m.model
                ));
            };
            if models
                .insert(m.model, (prototype, w.get::<Name>(e).cloned()))
                .is_some()
            {
                return Err(format!("grass model {} is placed twice", m.model));
            }
        }
        let terrains: Vec<Entity> = w.iter::<AnvilTerrainGroups>().map(|(e, _)| e).collect();
        let [x0, y0, x1, y1] = self.rect;
        let mut made: Vec<(Entity, Transform, Option<Name>)> = Vec::new();
        let (mut samples, mut with_group, mut bad_group, mut no_weight) = (0u64, 0u64, 0u64, 0u64);
        let (mut bad_model, mut no_placement) = (0u64, 0u64);
        let mut per_model: BTreeMap<u8, u64> = BTreeMap::new();
        let mut plants = 0u64;
        for te in terrains {
            let (Some(groups), Some(terrain)) = (w.get::<AnvilTerrainGroups>(te), w.get::<AnvilTerrain>(te))
            else {
                return Err("a terrain holds grass rasters without its heights".to_owned());
            };
            let level = terrain.level.min(30);
            let node_side = terrain.side / f64::from(1u32 << level);
            let spacing = node_side / f64::from(TERRAIN_CORE);
            let shift = match self.sample {
                CellPoint::Corner => 0.0,
                CellPoint::Centre => spacing / 2.0,
            };
            let unit = self.range * 2f64.powi(level as i32 - 28);
            let frame = groups.frame;
            let heights: BTreeMap<(u32, u32), &TerrainTile> =
                terrain.tiles.iter().map(|t| ((t.row, t.column), t)).collect();
            for gt in &groups.tiles {
                let GroupTile {
                    row,
                    column,
                    group,
                    weight,
                } = gt;
                let Some(&tile) = heights.get(&(*row, *column)) else {
                    log.omit("terrain nodes with grass rasters and no heights", 1);
                    continue;
                };
                if tile.heights.len() != TERRAIN_TILE * TERRAIN_TILE
                    || group.len() != TERRAIN_TILE * TERRAIN_TILE
                    || weight.len() != TERRAIN_TILE * TERRAIN_TILE
                {
                    log.omit("terrain nodes whose rasters are not whole", 1);
                    continue;
                }
                let ground = Ground {
                    tile,
                    unit,
                    offset: self.offset,
                    spacing,
                };
                let (nx, ny) = (
                    terrain.origin[0] + node_side * f64::from(*column),
                    terrain.origin[1] + node_side * f64::from(*row),
                );
                if nx > x1 || nx + node_side < x0 || ny > y1 || ny + node_side < y0 {
                    continue;
                }
                for i in 0..TERRAIN_CORE {
                    for j in 0..TERRAIN_CORE {
                        let at = (i as usize + TerrainTile::CORE_AT) * TERRAIN_TILE
                            + j as usize
                            + TerrainTile::CORE_AT;
                        let (g, wt) = (group[at], weight[at]);
                        samples += 1;
                        if g == 254 || g == 255 {
                            continue;
                        }
                        if wt >> 1 == 0 {
                            no_weight += 1;
                            continue;
                        }
                        let Some(g_rules) = library.groups.get(usize::from(g)) else {
                            bad_group += 1;
                            continue;
                        };
                        with_group += 1;
                        let (gx, gy) = (column * TERRAIN_CORE + j, row * TERRAIN_CORE + i);
                        for (k, rule) in g_rules.rules.iter().enumerate() {
                            let Ok(model) = u8::try_from(rule.model) else {
                                bad_model += 1;
                                continue;
                            };
                            let Some(settings) = library.models.get(usize::from(model)) else {
                                bad_model += 1;
                                continue;
                            };
                            let Some((prototype, name)) = models.get(&model) else {
                                no_placement += 1;
                                continue;
                            };
                            let mut rng = Rng::new(self.seed, gx, gy, g, k);
                            let candidates = if self.tiling {
                                4u64.checked_pow(u32::from(rule.tiling_log_count)).ok_or_else(|| {
                                    format!(
                                        "grass rule {:#x} of group {g}: TilingLogCount {} makes more candidates than a u64 holds",
                                        rule.id, rule.tiling_log_count
                                    )
                                })?
                            } else {
                                1
                            };
                            if candidates > self.max_plants {
                                return Err(format!(
                                    "grass rule {:#x} of group {g}: TilingLogCount {} makes {candidates} candidates a sample, over max-plants {}",
                                    rule.id, rule.tiling_log_count, self.max_plants
                                ));
                            }
                            for _ in 0..candidates {
                                let count = match self.density {
                                    Density::PerSample => u32::from(rng.unit() < f64::from(rule.density)),
                                    Density::PerSquareMetre => {
                                        let lambda = f64::from(rule.density) * spacing * spacing;
                                        lambda.floor() as u32 + u32::from(rng.unit() < lambda.fract())
                                    }
                                };
                                for _ in 0..count {
                                    // the plant's place: the sample point and a uniform offset within half a sample
                                    let (fi, fj) = (
                                        f64::from(i) + rng.between(-0.5, 0.5),
                                        f64::from(j) + rng.between(-0.5, 0.5),
                                    );
                                    let (x, y) = (nx + fj * spacing + shift, ny + fi * spacing + shift);
                                    if !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                                        continue;
                                    }
                                    plants += 1;
                                    if plants > self.max_plants {
                                        return Err(format!(
                                            "the options make more than max-plants {} plants over the rectangle",
                                            self.max_plants
                                        ));
                                    }
                                    let (z, n) = ground.at(fi, fj)?;
                                    let yaw = rng.between(0.0, std::f64::consts::TAU);
                                    let s = if self.scale && rule.scaling_enabled {
                                        let (a, b) = (f64::from(rule.scale_a), f64::from(rule.scale_b));
                                        rng.between(a.min(b), a.max(b))
                                    } else {
                                        1.0
                                    };
                                    let hs = if self.height_scale && rule.height_scaling_enabled {
                                        let (a, b) =
                                            (f64::from(rule.height_scale_a), f64::from(rule.height_scale_b));
                                        rng.between(a.min(b), a.max(b))
                                    } else {
                                        1.0
                                    };
                                    let mut up = [0.0, 0.0, 1.0];
                                    if self.normal_terrain {
                                        let r = f64::from(settings.settings.terrain_normal_follower_ratio)
                                            .clamp(0.0, 1.0);
                                        up = norm(add(mul(up, 1.0 - r), mul(n, r)));
                                    }
                                    if self.tilt {
                                        let angle = rng.between(0.0, f64::from(rule.tilt_factor));
                                        let phi = rng.between(0.0, std::f64::consts::TAU);
                                        let side = norm(cross(up, [phi.cos(), phi.sin(), 0.0]));
                                        let around = cross(up, side);
                                        up = norm(add(
                                            up,
                                            mul(
                                                add(mul(side, phi.cos()), mul(around, phi.sin())),
                                                angle.tan(),
                                            ),
                                        ));
                                    }
                                    let t =
                                        plant(yaw, up, s, hs, [x as f32, y as f32, z as f32].map(f64::from));
                                    *per_model.entry(model).or_default() += 1;
                                    made.push((*prototype, frame.transform(&t), name.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (prototype, transform, name) in made {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            w.insert(e, transform);
            if let Some(name) = name {
                w.insert(e, name);
            }
            log.place(e);
        }
        // the setup, the models' own placements and the grass rasters are consumed
        primase_core::ledger::despawn(w, setup, log, "the grass setup: the scatter reads it");
        for e in placements {
            primase_core::ledger::despawn(
                w,
                e,
                log,
                "a grass model's own placement: the scatter replaces it",
            );
        }
        let with_groups: Vec<Entity> = w.iter::<AnvilTerrainGroups>().map(|(e, _)| e).collect();
        for e in with_groups {
            w.remove::<AnvilTerrainGroups>(e);
        }
        log.count(
            "grass plants placed by a stand-in for the engine's run-time scatter",
            plants,
        );
        log.count("ground samples read for grass", samples);
        log.count("ground samples with a group and a weight", with_group);
        for (m, n) in per_model {
            log.count(&format!("grass plants of model {m}"), n);
        }
        log.omit("grass samples whose group is outside the library", bad_group);
        log.omit("grass samples with a zero weight", no_weight);
        log.omit(
            "grass rules naming a model the library does not have (46 to 50 of 46)",
            bad_model,
        );
        log.omit(
            "grass rules naming a model with no placement in the world",
            no_placement,
        );
        log.omit(
            "the engine's draw distance, LOD switch distances, dither fade, wind and bend (engine-bound), and its \
             camera-dependent selection",
            plants,
        );
        Ok(())
    }
}
