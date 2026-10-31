//! The pass that lowers the area's map collision (`fromsoft.map_collision`): which parts are ground, what each
//! HitMtrl class becomes and how far around the read region to keep are `lower-fromsoft-collision`'s options. Raw
//! collision is never roof-smoothed here.
use super::settings::Metres;
use crate::sekiro::source::{CollisionSource, MapCollision, ReadScope};
use fromsoft_formats::Game;
use fromsoft_formats::collision::{HitFilterMeaning, HitMeaning};
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
    named_surface(fromsoft_formats::collision::hit_meaning(game, class))
}

/// Destination vocabulary follows named authored meanings; no game IDs are chosen by the pass.
fn named_surface(meaning: HitMeaning) -> Option<&'static str> {
    Some(match meaning {
        HitMeaning::Stone => "concrete",
        HitMeaning::Earth => "earth",
        HitMeaning::Wood => "wood",
        HitMeaning::Grass => "grass",
        HitMeaning::Gravel => "gravel",
        HitMeaning::Metal => "metal",
        HitMeaning::Sand => "sand",
        HitMeaning::RoofTile => "roofing",
        HitMeaning::Ice => "marble",
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
pub(super) fn surface_of(game: Game, material: i32, meaning: HitMeaning, log: &mut PassLog) -> &'static str {
    if material < 0 {
        log.approximate(
            "triangles whose shape key binds no HitMtrl, given the default surface",
            1,
        );
        return "default";
    }
    let class = material % 100;
    named_surface(meaning).unwrap_or_else(|| {
        log.approximate(
            "triangles of HitMtrl classes without a known surface, given the default surface",
            1,
        );
        log.count(
            &match class_name(game, class) {
                Some(name) => {
                    format!("triangles of HitMtrl class {class} ({name}) without a known surface")
                }
                None => {
                    format!("triangles of HitMtrl class {class} (no name) without a known surface")
                }
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

/// Lower the map collision whose parts the reader names as physical. Authored swimming water forms
/// water soups; Empty/Cloud do not form solid geometry. Other and unknown meanings stay ground,
/// with unsupported surfaces counted. The real choices are the crop and whether to include f meshes.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftCollision {
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

impl LowerFromSoftCollision {
    /// From its declared options.
    fn from_given(g: &Given) -> std::result::Result<Self, String> {
        Ok(LowerFromSoftCollision {
            reach: Metres::parse_or_none("reach", g.required("reach"))?,
            f_meshes: g.optional_choice::<FMeshes>("f-meshes").unwrap_or(FMeshes::Off),
        })
    }
}

/// `lower-fromsoft-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): every choice is the
/// program's, stated.
pub const LOWER_FROMSOFT_COLLISION: PassSpec = PassSpec {
    name: "lower-fromsoft-collision",
    options: &[
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
        let mut options = Options::from([("reach".to_owned(), Metres::text_or_none(self.reach))]);
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
                .filter(|p| p.filter_meaning == HitFilterMeaning::Physical)
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
            if record.material_meanings.len() != record.materials.len()
                || record.triangles.len() != record.materials.len()
            {
                return Err(format!(
                    "{}: triangles, material IDs and named meanings differ in length",
                    record.file
                ));
            }
            for ((vertices, &material), &meaning) in record
                .triangles
                .iter()
                .zip(&record.materials)
                .zip(&record.material_meanings)
            {
                if crop.is_some_and(|c| beyond(vertices, c)) {
                    log.count("triangles beyond the reach around the read region, left out", 1);
                    continue;
                }
                match meaning {
                    HitMeaning::SwimmingWater => {
                        push(water, *vertices, index("default")?)?;
                        log.count("water triangles", 1);
                    }
                    HitMeaning::Empty | HitMeaning::Cloud => {
                        log.count("triangles of left-out HitMtrl classes", 1)
                    }
                    _ => {
                        push(solid, *vertices, index(surface_of(game, material, meaning, log))?)?;
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
                filter_meaning: fromsoft_formats::collision::hit_filter_meaning(Game::Sekiro, filter),
                condition: None,
            }],
            triangles,
            f_triangles: Vec::new(),
            material_meanings: materials
                .iter()
                .map(|&id| fromsoft_formats::collision::hit_meaning(Game::Sekiro, id))
                .collect(),
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
    fn unknown_material_ids_stay_authored_and_their_triangles_are_reported() {
        let mut w = World::new();
        let e = w.spawn();
        let record = record("unknown.hkx", 8, vec![NEAR], vec![99]);
        assert_eq!(record.materials, [99]);
        assert_eq!(record.material_meanings, [HitMeaning::Unknown]);
        w.insert(e, record);
        let mut log = PassLog::default();
        LowerFromSoftCollision {
            reach: None,
            f_meshes: FMeshes::Off,
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert_eq!(soups(&w)["game"], ["default"]);
        assert_eq!(log.counts["ground triangles"], 1);
        assert_eq!(
            log.counts["triangles of HitMtrl class 99 (no name) without a known surface"],
            1
        );
    }

    #[test]
    fn the_program_chooses_filters_classes_and_reach_and_other_filters_stay_as_source() {
        let mut w = world();
        let pass = LOWER_FROMSOFT_COLLISION
            .instance(&Options::from([("reach".into(), "50".into())]))
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
            let mut record = record("a.hkx", 8, triangles.clone(), materials.clone());
            record.material_meanings = materials
                .iter()
                .map(|&id| fromsoft_formats::collision::hit_meaning(game, id))
                .collect();
            record.material_meanings[2] = HitMeaning::SwimmingWater; // synthetic fixture, not Sekiro's class-21 meaning
            w.insert(e, record);
            w.insert_resource(ReadScope {
                game,
                area: "m10_00_00_00".into(),
                region: None,
                states: false,
            });
            let pass = LowerFromSoftCollision {
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
            reach: None,
            f_meshes: FMeshes::Off,
        };
        let mut quiet = PassLog::default();
        only_8.run(&mut w.clone(), &mut quiet).unwrap();
        assert!(quiet.omitted.is_empty(), "{:?}", quiet.omitted);
        let with_15 = LowerFromSoftCollision {
            reach: None,
            f_meshes: FMeshes::Off,
        };
        w.get_mut::<MapCollision>(b).unwrap().parts[0].filter_meaning = HitFilterMeaning::Physical;
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
            w.get_mut::<MapCollision>(a)
                .unwrap()
                .material_meanings
                .fill(HitMeaning::Other);
            w
        };
        let options = |f: Option<&str>| {
            let mut o = Options::from([("reach".into(), "50".into())]);
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
