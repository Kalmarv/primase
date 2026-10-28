//! The pass that lowers the area's map collision (`fromsoft.map_collision`): which parts are ground, what each
//! HitMtrl class becomes and how far around the read region to keep are `lower-fromsoft-collision`'s options. Raw
//! collision is never roof-smoothed here.
use super::settings::{HitClass, HitFilters, Metres, joined};
use crate::sekiro::source::{CollisionSource, MapCollision, ReadScope};
use fromsoft_formats::Game;
use primase_core::convert::PartScope;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::{Clause, Condition};
use primase_core::{Collision, Component, Entity, Name, Surfaces, Transform, World, world::Resource};
use std::collections::{BTreeMap, BTreeSet};

/// The game whose HitMtrl classes the world's collision uses: the one its read states (`fromsoft.read_scope`); a world
/// without a scope (one a test builds by hand) reads them as Sekiro's, the table the passes began with.
pub(super) fn game_of(world: &World) -> Game {
    world
        .resource::<ReadScope>()
        .map_or(Game::Sekiro, |scope| scope.game)
}

/// The core surface a HitMtrl class (an ID's base value, modulo 100) of `game` stands for, where its class is known to
/// stand for one; `None` for the other classes.
pub fn known_surface(game: Game, class: i32) -> Option<&'static str> {
    match game {
        Game::Sekiro => sekiro_surface(class),
        Game::EldenRing => elden_surface(class),
    }
}

/// Sekiro's classes by the names of its HitMtrlParam rows (Paramdex, as Smithbox carries it): Stone and Underwater
/// Stone concrete; Dirt, Ash, Dead Leaves, Mud, Snow, Autumn Leaves and Underwater Dirt earth; Wood, Bamboo, Tree,
/// Tatami, Wood Indoors and Underwater Wood wood; Grass, Straw, Bush and the grasses in water grass; Gravel; Iron and
/// Bell metal; Sand; Tiles roofing; Ice marble.
fn sekiro_surface(class: i32) -> Option<&'static str> {
    Some(match class {
        1 | 46 => "concrete",
        3 | 15 | 18 | 27 | 30 | 33 | 45 => "earth",
        4 | 7 | 8 | 28 | 32 | 47 => "wood",
        5 | 9 | 10 | 31 | 37 | 38 => "grass",
        6 => "gravel",
        11 | 19 => "metal",
        13 => "sand",
        16 => "roofing",
        34 => "marble",
        _ => return None,
    })
}

/// Elden Ring's classes by the names of its HitMtrlParam rows (the game's param names as Smithbox carries them, row IDs
/// 0 to 62; its numbering is not Sekiro's: 3 is Sand here and Dirt there). A class is mapped where its name says a
/// surface of the core table: Rock the nearest, concrete; Sand; Wood and the Scarlet Tree wood; Dirt and Rain Dirt
/// earth; Iron Grate metal. Not mapped, because the name is not a surface of the table or there is none: Stock (1), Ore
/// (6), Lava (7), Scarlet Mushroom (12), Bone (14), the swamps and the Lake of Rot (23, 24, 46, 49), and every row the
/// names leave blank. Water (21) is not a surface: the water classes are the program's (`water=`).
fn elden_surface(class: i32) -> Option<&'static str> {
    Some(match class {
        2 => "concrete",
        3 => "sand",
        4 | 8 => "wood",
        5 | 44 => "earth",
        11 => "metal",
        _ => return None,
    })
}

/// The name of a HitMtrl class's row in `game`'s HitMtrlParam (the community's English names: Smithbox's, Paramdex's for
/// Sekiro), where it has one: what a counted class is called. The table is `fromsoft_formats::collision::hit_class_name`'s.
pub fn class_name(game: Game, class: i32) -> Option<&'static str> {
    fromsoft_formats::collision::hit_class_name(game, class)
}

/// The surface a triangle of HitMtrl ID `material` is lowered with in `game`: its class's known surface, else `default`,
/// counted as approximations in `log`; a class without a known surface is also counted by itself and its name
/// (`triangles of HitMtrl class 12 (Scarlet Mushroom) without a known surface`), never given the default unseen.
pub(super) fn surface_of(game: Game, material: i32, log: &mut PassLog) -> &'static str {
    if material < 0 {
        log.approximate(
            "triangles whose shape key binds no HitMtrl, given the default surface",
            1,
        );
        return "default";
    }
    let class = material % 100;
    known_surface(game, class).unwrap_or_else(|| {
        log.approximate(
            "triangles of HitMtrl classes without a known surface, given the default surface",
            1,
        );
        log.count(
            &match class_name(game, class) {
                Some(name) => format!("triangles of HitMtrl class {class} ({name}) without a known surface"),
                None => format!("triangles of HitMtrl class {class} (no name) without a known surface"),
            },
            1,
        );
        "default"
    })
}

pub(super) fn push(
    soup: &mut Collision,
    vertices: [[f32; 3]; 3],
    surface: u16,
) -> std::result::Result<(), String> {
    let start = u32::try_from(soup.positions.len()).map_err(|_| "too many soup vertices".to_owned())?;
    if start > u32::MAX - 3 {
        return Err("soup index overflow".into());
    }
    soup.positions.extend(vertices);
    // the world frame reflects the source's: the stored order faces away
    soup.triangles.push([start, start + 2, start + 1]);
    soup.surfaces.push(surface);
    Ok(())
}

/// A non-empty soup as its own entity, with the group it was lowered into; with `condition`, present only while it
/// holds (the entity carries it and an identity transform: state sits on placed things).
pub(super) fn attach(
    world: &mut World,
    soup: Collision,
    group: &str,
    water: bool,
    condition: Option<&Condition>,
    log: &mut PassLog,
) -> Option<Entity> {
    if soup.triangles.is_empty() {
        return None;
    }
    let entity = world.spawn();
    world.insert(entity, soup);
    world.insert(entity, Name(group.to_owned()));
    world.insert(
        entity,
        CollisionSource {
            group: group.to_owned(),
            water,
        },
    );
    if let Some(c) = condition {
        world.insert(entity, c.clone());
        world.insert(entity, Transform::IDENTITY);
    }
    log.place(entity);
    Some(entity)
}

/// The entities whose input a pass lowers: in a part of a conversion (`core.part_scope`), what the part's roots
/// reach, as what it only loaded as context is its owner's to lower; without one, everything (`None`).
pub(super) fn owned(world: &World) -> Option<BTreeSet<Entity>> {
    world
        .resource::<PartScope>()
        .map(|s| world.closure(s.roots.iter().copied()))
}

/// The read region around which a pass crops at `reach`, if it crops: `None` without a reach or after a read of the
/// whole area.
pub(super) fn crop(
    world: &World,
    reach: Option<Metres>,
) -> std::result::Result<Option<([f32; 4], f32)>, String> {
    let Some(reach) = reach.map(Metres::get) else {
        return Ok(None);
    };
    let scope = world
        .resource::<ReadScope>()
        .ok_or("reach crops around the read region: the world has no fromsoft.read_scope")?;
    Ok(scope.region.map(|r| (r.rect, reach)))
}

/// Whether a world-frame triangle's centre lies beyond `reach` metres outside the east/north rectangle `r`.
pub(super) fn beyond(v: &[[f32; 3]; 3], (r, reach): ([f32; 4], f32)) -> bool {
    let x = (v[0][0] + v[1][0] + v[2][0]) / 3.;
    let z = -(v[0][2] + v[1][2] + v[2][2]) / 3.;
    x < r[0] - reach || x >= r[2] + reach || z < r[1] - reach || z >= r[3] + reach
}

/// `lower-fromsoft-collision{hit-filters=...}`: the map collision files (`fromsoft.map_collision`) that parts of the
/// chosen hit filters name, as `core.collision` soups, the records consumed. Every triangle goes into the `game` soup
/// with its HitMtrl class's surface ([`known_surface`]; an ID the shape key does not bind, or a class without a known
/// surface, gets `default`, counted); a file present only in some world states (its selected parts' clauses) goes
/// into a `game_state<n>` soup with that condition. Files only parts of other hit filters name stay as source,
/// counted. In a part of a conversion (`core.part_scope`), only the files its roots reach are lowered: a file it
/// loaded as context is its own part's.
///
/// Every field is the program's choice, stated (no default): `hit_filters`, the filters whose parts are lowered (which
/// placed collision is ground); `water`, HitMtrl classes lowered into separate `water` soups with the `default`
/// surface; `leave_out`, classes not lowered, counted (a class in both is refused); `reach`, leaving out, counted, the
/// triangles whose centre lies more than that many metres outside the read region (`fromsoft.read_scope`), or `None`
/// for no crop; `f-meshes`, whether the `f` binder's meshes the read brought in with the files
/// ([`MapCollision::f_triangles`]; `Options::f_collision`) are lowered, into a soup of their own (`f`, `f_state<n>`
/// with the file's condition) with the `default` surface, never into `game`: they store no HitMtrl, and are not known
/// to be ground. Optional: absent is `off`, and they are counted as left out. Options, all required but `f-meshes`:
/// `hit-filters=` and `water=`/`leave-out=` classes joined by `+` (empty: none),
/// `reach=N|none`.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftCollision {
    pub hit_filters: HitFilters,
    pub water: BTreeSet<HitClass>,
    pub leave_out: BTreeSet<HitClass>,
    pub reach: Option<Metres>,
    pub f_meshes: FMeshes,
}

/// Whether `lower-fromsoft-collision` lowers the `f` binder's meshes (`f-meshes=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FMeshes {
    /// Not lowered, counted.
    Off,
    /// Lowered into the `f` soups.
    On,
}

impl primase_core::pass::Choice for FMeshes {
    const VALUES: &'static [(&'static str, Self)] = &[("off", FMeshes::Off), ("on", FMeshes::On)];
}

/// What a HitMtrl class's triangles become instead of ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassChoice {
    /// The `water` soups, with the `default` surface.
    Water,
    /// Not lowered, counted.
    LeaveOut,
}

impl LowerFromSoftCollision {
    /// From its declared options.
    fn from_given(g: &Given) -> std::result::Result<Self, String> {
        Ok(LowerFromSoftCollision {
            hit_filters: HitFilters::parse("hit-filters", g.required("hit-filters"))?,
            water: HitClass::parse_set("water", g.required("water"))?,
            leave_out: HitClass::parse_set("leave-out", g.required("leave-out"))?,
            reach: Metres::parse_or_none("reach", g.required("reach"))?,
            f_meshes: g.optional_choice::<FMeshes>("f-meshes").unwrap_or(FMeshes::Off),
        })
    }

    /// What becomes of a class's triangles instead of ground, refusing a class both water and left out.
    fn classes(&self) -> std::result::Result<BTreeMap<HitClass, ClassChoice>, String> {
        if let Some(c) = self.water.intersection(&self.leave_out).next() {
            return Err(format!("HitMtrl class {c} is both water and left out"));
        }
        let water = self.water.iter().map(|&c| (c, ClassChoice::Water));
        Ok(water
            .chain(self.leave_out.iter().map(|&c| (c, ClassChoice::LeaveOut)))
            .collect())
    }
}

/// `lower-fromsoft-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): every choice is the
/// program's, stated.
pub const LOWER_FROMSOFT_COLLISION: PassSpec = PassSpec {
    name: "lower-fromsoft-collision",
    options: &[
        OptionSpec::required(
            "hit-filters",
            "8+27",
            "the hit filters whose collision parts are lowered as ground, joined by + (at least one)",
        ),
        OptionSpec::required(
            "water",
            "22+36",
            "HitMtrl classes (0 to 99) lowered into separate water soups, joined by + (empty for none)",
        ),
        OptionSpec::required(
            "leave-out",
            "29+40",
            "HitMtrl classes (0 to 99) not lowered (counted), joined by + (empty for none)",
        ),
        OptionSpec::required(
            "reach",
            "50",
            "metres, at least 0: the triangles whose centre lies further outside the read region are left out \
             (counted), or none for no crop",
        ),
        OptionSpec::optional_choice::<FMeshes>(
            "f-meshes",
            "lower the f collision binder's meshes (read with Options::f_collision) into soups of their own",
            "off: they are left out, counted",
        ),
    ],
    make: |g| Ok(Box::new(LowerFromSoftCollision::from_given(g)?)),
};

impl Pass for LowerFromSoftCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-collision"
    }
    fn options(&self) -> Options {
        let mut options = Options::from([
            ("hit-filters".to_owned(), self.hit_filters.to_string()),
            ("water".to_owned(), joined(self.water.iter().copied())),
            ("leave-out".to_owned(), joined(self.leave_out.iter().copied())),
            ("reach".to_owned(), Metres::text_or_none(self.reach)),
        ]);
        // an option a program may leave out is printed only when it is stated, so the pipelines that never state it print as before
        if self.f_meshes == FMeshes::On {
            options.insert("f-meshes".to_owned(), self.f_meshes.text().to_owned());
        }
        options
    }
    fn requires(&self) -> &[&'static str] {
        &[MapCollision::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Collision::NAME,
            CollisionSource::NAME,
            Name::NAME,
            Condition::NAME,
            Transform::NAME,
            Surfaces::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[MapCollision::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "triangles whose shape key binds no HitMtrl, HitMtrl classes without a known surface and the f binder's meshes          (which store no HitMtrl), as the default surface"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let crop = crop(world, self.reach)?;
        let classes = self.classes()?;
        let game = game_of(world);
        let owned = owned(world);
        let mut records: Vec<(Entity, &MapCollision)> = world
            .iter::<MapCollision>()
            .filter(|(e, _)| {
                let mine = owned.as_ref().is_none_or(|o| o.contains(e));
                if !mine {
                    log.count("context collision files, left to their own part", 1);
                }
                mine
            })
            .collect();
        records.sort_by(|a, b| a.1.file.cmp(&b.1.file));
        let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
        let mut index = |name: &str| surfaces.index(name).map_err(|e| e.to_string());
        let mut solid = Collision::default();
        let mut water = Collision::default();
        let mut f_soup = Collision::default();
        // soups of files present only in some states, by condition: ground, water and f meshes
        let mut conditioned: BTreeMap<String, (Condition, Collision, Collision, Collision)> = BTreeMap::new();
        let mut consumed = Vec::new();
        for (e, record) in records {
            let clauses: Vec<Option<Clause>> = record
                .parts
                .iter()
                .filter(|p| self.hit_filters.contains(p.hit_filter))
                .map(|p| p.condition.clone())
                .collect();
            if !clauses.is_empty() {
                // a body of another shape class (a convex polytope) stores no HitMtrl material and has no triangles
                // here: it is left out, each class counted, while the file's mesh bodies are lowered; never taken for
                // an empty body, and never a reason to refuse the file's other bodies
                for shape in &record.unread_bodies {
                    log.omit(
                        &format!(
                            "collision bodies of {shape}, a shape the reader does not decode, in a chosen hit filter's file"
                        ),
                        1,
                    );
                }
            }
            if clauses.is_empty() {
                log.count(
                    "collision files only other hit filters' parts name, left as source",
                    1,
                );
                continue;
            }
            let (solid, water, f_soup) = match super::any_of(&clauses) {
                None => (&mut solid, &mut water, &mut f_soup),
                Some(c) => {
                    let entry = conditioned.entry(super::condition_key(&c)).or_insert_with(|| {
                        (
                            c,
                            Collision::default(),
                            Collision::default(),
                            Collision::default(),
                        )
                    });
                    (&mut entry.1, &mut entry.2, &mut entry.3)
                }
            };
            for (vertices, &material) in record.triangles.iter().zip(&record.materials) {
                if crop.is_some_and(|c| beyond(vertices, c)) {
                    log.count("triangles beyond the reach around the read region, left out", 1);
                    continue;
                }
                match HitClass::of(material).and_then(|c| classes.get(&c)) {
                    Some(ClassChoice::Water) => {
                        push(water, *vertices, index("default")?)?;
                        log.count("water triangles", 1);
                    }
                    Some(ClassChoice::LeaveOut) => log.count("triangles of left-out HitMtrl classes", 1),
                    None => {
                        push(solid, *vertices, index(surface_of(game, material, log))?)?;
                        log.count("ground triangles", 1);
                    }
                }
            }
            for vertices in &record.f_triangles {
                if self.f_meshes == FMeshes::Off {
                    log.count("f-mesh triangles of lowered files, left out (f-meshes=off)", 1);
                } else if crop.is_some_and(|c| beyond(vertices, c)) {
                    log.count("triangles beyond the reach around the read region, left out", 1);
                } else {
                    push(f_soup, *vertices, index("default")?)?;
                    log.approximate(
                        "f-mesh triangles, which store no HitMtrl, given the default surface",
                        1,
                    );
                    log.count("f-mesh triangles", 1);
                }
            }
            log.count("collision files lowered", 1);
            consumed.push(e);
        }
        for e in consumed {
            world.despawn(e);
        }
        world.insert_resource(surfaces);
        attach(world, solid, "game", false, None, log);
        attach(world, water, "water", true, None, log);
        attach(world, f_soup, "f", false, None, log);
        for (n, (condition, solid, water, f_soup)) in conditioned.into_values().enumerate() {
            log.count("soups present only in some world states", 1);
            attach(
                world,
                solid,
                &format!("game_state{n}"),
                false,
                Some(&condition),
                log,
            );
            attach(
                world,
                water,
                &format!("water_state{n}"),
                true,
                Some(&condition),
                log,
            );
            attach(
                world,
                f_soup,
                &format!("f_state{n}"),
                false,
                Some(&condition),
                log,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::source::CollisionPart;

    fn record(file: &str, filter: u8, triangles: Vec<[[f32; 3]; 3]>, materials: Vec<i32>) -> MapCollision {
        MapCollision {
            file: file.into(),
            parts: vec![CollisionPart {
                name: format!("{file}-part"),
                hit_filter: filter,
                condition: None,
            }],
            triangles,
            f_triangles: Vec::new(),
            materials,
            invalid_material_keys: 0,
            dead_primitives: 0,
            unread_bodies: Vec::new(),
        }
    }

    const NEAR: [[f32; 3]; 3] = [[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]];
    const FAR: [[f32; 3]; 3] = [[500., 0., 0.], [501., 0., 0.], [500., 0., -1.]];

    /// One file of filter 8 with ground, water, a blocker, an unbound and a far triangle; one of filter 15.
    fn world() -> World {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(
            a,
            record(
                "a.hkx",
                8,
                vec![NEAR, NEAR, NEAR, NEAR, FAR],
                vec![104, 22, 29, -1, 1],
            ),
        );
        let b = w.spawn();
        w.insert(b, record("b.hkx", 15, vec![NEAR], vec![1]));
        w.insert_resource(ReadScope {
            game: Game::Sekiro,
            area: "m00_00_00_00".into(),
            region: Some(crate::sekiro::source::Region {
                rect: [-10., -10., 10., 10.],
                margin: 30.,
            }),
            states: false,
        });
        w
    }

    fn soups(w: &World) -> BTreeMap<String, Vec<String>> {
        let surfaces = w.resource::<Surfaces>().unwrap();
        w.iter::<Collision>()
            .map(|(e, c)| {
                (
                    w.get::<CollisionSource>(e).unwrap().group.clone(),
                    c.surfaces
                        .iter()
                        .map(|&s| surfaces.0[usize::from(s)].name.clone())
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_program_chooses_filters_classes_and_reach_and_other_filters_stay_as_source() {
        let mut w = world();
        let pass = LOWER_FROMSOFT_COLLISION
            .instance(&Options::from([
                ("hit-filters".into(), "8".into()),
                ("water".into(), "22".into()),
                ("leave-out".into(), "29".into()),
                ("reach".into(), "50".into()),
            ]))
            .unwrap();
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        assert_eq!(
            soups(&w),
            BTreeMap::from([
                ("game".into(), vec!["wood".into(), "default".into()]),
                ("water".into(), vec!["default".into()]),
            ])
        );
        assert_eq!(log.counts["triangles of left-out HitMtrl classes"], 1);
        assert_eq!(
            log.counts["triangles beyond the reach around the read region, left out"],
            1
        );
        assert_eq!(
            log.approximations["triangles whose shape key binds no HitMtrl, given the default surface"],
            1
        );
        // the filter-15 file is still the source record, whole
        let left: Vec<&MapCollision> = w.iter::<MapCollision>().map(|(_, m)| m).collect();
        assert_eq!(left.len(), 1);
        assert_eq!(
            (left[0].file.as_str(), left[0].parts[0].hit_filter),
            ("b.hkx", 15)
        );
    }

    #[test]
    fn with_no_classes_and_no_reach_stated_every_triangle_of_the_chosen_files_is_ground() {
        let options = |water: &str, leave_out: &str| {
            Options::from([
                ("hit-filters".into(), "8+15".into()),
                ("water".into(), water.into()),
                ("leave-out".into(), leave_out.into()),
                ("reach".into(), "none".into()),
            ])
        };
        let mut w = world();
        let pass = LOWER_FROMSOFT_COLLISION.instance(&options("", "")).unwrap();
        assert_eq!(pass.options(), options("", ""));
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        assert_eq!(soups(&w)["game"].len(), 6);
        assert!(w.iter::<MapCollision>().next().is_none());
        // every option is the program's to state; a class cannot be both water and left out
        assert!(LOWER_FROMSOFT_COLLISION.instance(&Options::new()).is_err());
        assert!(
            LOWER_FROMSOFT_COLLISION
                .instance(&Options::from([("hit-filters".into(), "8".into())]))
                .is_err()
        );
        let both = LOWER_FROMSOFT_COLLISION.instance(&options("22", "22")).unwrap();
        let err = both.run(&mut world(), &mut PassLog::default()).unwrap_err();
        assert!(err.contains("both water and left out"), "{err}");
    }

    /// A selection resolves state first (`resolve-state`, then the lowering): the clauses the parts hold inside their record
    /// are reduced by it as `core.condition` is, so the lowering copies none on an axis that is gone. A file whose only
    /// part needs the flag on (off by default) is absent, a file whose part needs it off is present with no condition, and
    /// no soup is left conditioned on a removed axis.
    #[test]
    fn the_parts_clauses_are_reduced_when_state_is_resolved_before_the_lowering() {
        use primase_core::state::{Domain, Literal, ResolveState, Scope, StateAxis};
        let mut w = World::new();
        let flag = w.spawn();
        w.insert(
            flag,
            StateAxis {
                key: "fromsoft.flag.1".into(),
                domain: Domain::Bool,
                default: Some(0),
                scope: Scope::Global,
                class: "event".into(),
                source: "event flag 1".into(),
            },
        );
        let clause = |values| Some(Clause(vec![Literal { axis: flag, values }]));
        for (file, condition) in [
            ("on.hkx", clause(0b10)),
            ("off.hkx", clause(0b01)),
            ("always.hkx", None),
        ] {
            let e = w.spawn();
            let mut r = record(file, 8, vec![NEAR], vec![104]);
            r.parts[0].condition = condition;
            w.insert(e, r);
        }
        let mut log = PassLog::default();
        ResolveState::default().run(&mut w, &mut log).unwrap();
        let mut files: Vec<(String, Option<Clause>)> = w
            .iter::<MapCollision>()
            .map(|(_, m)| (m.file.clone(), m.parts[0].condition.clone()))
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        // the flag is off: the file needing it on is not present, the others hold whatever is left open (nothing)
        assert_eq!(
            files,
            [("always.hkx".to_owned(), None), ("off.hkx".to_owned(), None)]
        );
        let pass = LowerFromSoftCollision {
            hit_filters: HitFilters::new([8]).unwrap(),
            water: BTreeSet::new(),
            leave_out: BTreeSet::new(),
            reach: None,
            f_meshes: FMeshes::Off,
        };
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        assert_eq!(w.iter::<Condition>().count(), 0);
        // no water class chosen: one unconditioned ground soup, of the two files present (no empty water soup is made)
        assert_eq!(soups(&w).keys().cloned().collect::<Vec<_>>(), ["game".to_owned()]);
        let (_, game) = w.iter::<Collision>().next().unwrap();
        assert_eq!(game.triangles.len(), 2);
    }

    /// Elden Ring's classes are its own rows, not Sekiro's: 3 is Sand where Sekiro's is Dirt, 5 Dirt where Sekiro's is
    /// Grass, 21 Water. The world's read scope says which game's table a lowering reads; a class with no known
    /// surface is counted by itself, with its row's name where there is one.
    #[test]
    fn a_games_classes_are_its_own_and_an_unmapped_class_is_counted_by_name() {
        let materials = vec![3, 5, 21, 12, 2, 57, 1103];
        let triangles = vec![NEAR; materials.len()];
        let surfaces_of = |game: Game| {
            let mut w = World::new();
            let e = w.spawn();
            w.insert(e, record("a.hkx", 8, triangles.clone(), materials.clone()));
            w.insert_resource(ReadScope {
                game,
                area: "m10_00_00_00".into(),
                region: None,
                states: false,
            });
            let pass = LowerFromSoftCollision {
                hit_filters: HitFilters::new([8]).unwrap(),
                water: HitClass::set([21]).unwrap(),
                leave_out: BTreeSet::new(),
                reach: None,
                f_meshes: FMeshes::Off,
            };
            let mut log = PassLog::default();
            pass.run(&mut w, &mut log).unwrap();
            (soups(&w), log)
        };
        // 21 is the water class the program chose, in either game; 1103 is class 3: the ID modulo 100
        let (elden, log) = surfaces_of(Game::EldenRing);
        assert_eq!(
            elden["game"],
            ["sand", "earth", "default", "concrete", "default", "sand"],
            "3 Sand, 5 Dirt, 12 Scarlet Mushroom and 57 unmapped, 2 Rock, 1103 Sand"
        );
        assert_eq!(elden["water"], ["default"]);
        assert_eq!(log.counts["water triangles"], 1);
        assert_eq!(
            log.counts["triangles of HitMtrl class 12 (Scarlet Mushroom) without a known surface"],
            1
        );
        assert_eq!(
            log.counts["triangles of HitMtrl class 57 (no name) without a known surface"],
            1
        );
        // the same IDs in Sekiro: 3 Dirt, 5 Grass, 12 Flesh and 2 (Unused) unmapped, 1103 Dirt
        let (sekiro, _) = surfaces_of(Game::Sekiro);
        assert_eq!(
            sekiro["game"],
            ["earth", "grass", "default", "default", "default", "earth"]
        );
        assert_eq!(known_surface(Game::EldenRing, 11), Some("metal"));
        assert_eq!(known_surface(Game::Sekiro, 11), Some("metal"));
        assert_eq!(known_surface(Game::EldenRing, 21), None, "water is not a surface");
        assert_eq!(class_name(Game::EldenRing, 49), Some("Lake of Rot"));
        assert_eq!(class_name(Game::Sekiro, 22), Some("Water (Diveable)"));
    }

    #[test]
    fn a_chosen_file_with_bodies_the_reader_could_not_decode_is_lowered_without_them_and_says_so() {
        let mut w = world();
        let (b, _) = w.iter::<MapCollision>().find(|(_, m)| m.file == "b.hkx").unwrap();
        w.get_mut::<MapCollision>(b).unwrap().unread_bodies = vec!["hknpConvexShape".into()];
        let only_8 = LowerFromSoftCollision {
            hit_filters: HitFilters::new([8]).unwrap(),
            water: BTreeSet::new(),
            leave_out: BTreeSet::new(),
            reach: None,
            f_meshes: FMeshes::Off,
        };
        let mut quiet = PassLog::default();
        only_8.run(&mut w.clone(), &mut quiet).unwrap();
        assert!(quiet.omitted.is_empty(), "{:?}", quiet.omitted);
        let with_15 = LowerFromSoftCollision {
            hit_filters: HitFilters::new([8, 15]).unwrap(),
            water: BTreeSet::new(),
            leave_out: BTreeSet::new(),
            reach: None,
            f_meshes: FMeshes::Off,
        };
        let mut log = PassLog::default();
        with_15.run(&mut w, &mut log).unwrap();
        assert_eq!(
            log.omitted["collision bodies of hknpConvexShape, a shape the reader does not decode, in a chosen hit filter's file"],
            1
        );
        // the file's own triangles are lowered all the same
        assert_eq!(log.counts["collision files lowered"], 2);
    }

    /// A file's `f` meshes are lowered only when the program says so, into a soup of their own with the default surface:
    /// never into ground, cropped like the rest, and counted either way.
    #[test]
    fn the_f_binders_meshes_are_a_soup_of_their_own_when_the_program_asks_and_left_out_otherwise() {
        let with_f = || {
            let mut w = world();
            let (a, _) = w.iter::<MapCollision>().find(|(_, m)| m.file == "a.hkx").unwrap();
            w.get_mut::<MapCollision>(a).unwrap().f_triangles = vec![NEAR, NEAR, FAR];
            w
        };
        let options = |f: Option<&str>| {
            let mut o = Options::from([
                ("hit-filters".into(), "8".into()),
                ("water".into(), "".into()),
                ("leave-out".into(), "".into()),
                ("reach".into(), "50".into()),
            ]);
            if let Some(f) = f {
                o.insert("f-meshes".into(), f.into());
            }
            o
        };
        // absent is off: no f soup, the file's three f triangles counted as left out
        let mut w = with_f();
        let pass = LOWER_FROMSOFT_COLLISION.instance(&options(None)).unwrap();
        assert!(!pass.options().contains_key("f-meshes"));
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        assert!(!soups(&w).contains_key("f"));
        assert_eq!(
            log.counts["f-mesh triangles of lowered files, left out (f-meshes=off)"],
            3
        );
        // on: two f triangles inside the reach, one beyond it, all in the default surface and apart from the ground
        let mut w = with_f();
        let pass = LOWER_FROMSOFT_COLLISION.instance(&options(Some("on"))).unwrap();
        assert_eq!(pass.options()["f-meshes"], "on");
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        let soups = soups(&w);
        assert_eq!(soups["f"], ["default", "default"]);
        assert_eq!(
            soups["game"].len(),
            4,
            "the ground is what it was: 5 triangles, 1 beyond the reach"
        );
        assert_eq!(log.counts["f-mesh triangles"], 2);
        assert_eq!(
            log.approximations["f-mesh triangles, which store no HitMtrl, given the default surface"],
            2
        );
        // anything but off or on is refused
        assert!(
            LOWER_FROMSOFT_COLLISION
                .instance(&options(Some("maybe")))
                .is_err()
        );
    }
}
