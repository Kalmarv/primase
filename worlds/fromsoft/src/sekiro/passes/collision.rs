//! The pass that lowers the area's map collision (`fromsoft.map_collision`): which parts are ground, what each
//! HitMtrl class becomes and how far around the read region to keep are `lower-fromsoft-collision`'s options. Raw
//! collision is never roof-smoothed here.
use super::settings::{HitClass, HitFilters, Metres, joined};
use crate::sekiro::source::{CollisionSource, MapCollision, ReadScope};
use primase_core::convert::PartScope;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::{Clause, Condition};
use primase_core::{Collision, Component, Entity, Name, Surfaces, Transform, World, world::Resource};
use std::collections::{BTreeMap, BTreeSet};

/// The core surface a HitMtrl class (an ID's base value, modulo 100) stands for, where its class is known to stand
/// for one; `None` for the other classes.
pub fn known_surface(class: i32) -> Option<&'static str> {
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

/// The surface a triangle of HitMtrl ID `material` is lowered with: its class's known surface, else `default`,
/// counted as approximations in `log`.
pub(super) fn surface_of(material: i32, log: &mut PassLog) -> &'static str {
    if material < 0 {
        log.approximate(
            "triangles whose shape key binds no HitMtrl, given the default surface",
            1,
        );
        return "default";
    }
    known_surface(material % 100).unwrap_or_else(|| {
        log.approximate(
            "triangles of HitMtrl classes without a known surface, given the default surface",
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
/// for no crop. Options, all required: `hit-filters=` and `water=`/`leave-out=` classes joined by `+` (empty: none),
/// `reach=N|none`.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftCollision {
    pub hit_filters: HitFilters,
    pub water: BTreeSet<HitClass>,
    pub leave_out: BTreeSet<HitClass>,
    pub reach: Option<Metres>,
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
    ],
    make: |g| Ok(Box::new(LowerFromSoftCollision::from_given(g)?)),
};

impl Pass for LowerFromSoftCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("hit-filters".to_owned(), self.hit_filters.to_string()),
            ("water".to_owned(), joined(self.water.iter().copied())),
            ("leave-out".to_owned(), joined(self.leave_out.iter().copied())),
            ("reach".to_owned(), Metres::text_or_none(self.reach)),
        ])
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
        "triangles whose shape key binds no HitMtrl, and HitMtrl classes without a known surface, as the default surface"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let crop = crop(world, self.reach)?;
        let classes = self.classes()?;
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
        // soups of files present only in some states, by condition
        let mut conditioned: BTreeMap<String, (Condition, Collision, Collision)> = BTreeMap::new();
        let mut consumed = Vec::new();
        for (e, record) in records {
            let clauses: Vec<Option<Clause>> = record
                .parts
                .iter()
                .filter(|p| self.hit_filters.contains(p.hit_filter))
                .map(|p| p.condition.clone())
                .collect();
            if !clauses.is_empty() && !record.unread_bodies.is_empty() {
                return Err(format!(
                    "{}, of a chosen hit filter, has bodies of shapes the reader does not decode: {:?}",
                    record.file, record.unread_bodies
                ));
            }
            if clauses.is_empty() {
                log.count(
                    "collision files only other hit filters' parts name, left as source",
                    1,
                );
                continue;
            }
            let (solid, water) = match super::any_of(&clauses) {
                None => (&mut solid, &mut water),
                Some(c) => {
                    let entry = conditioned
                        .entry(super::condition_key(&c))
                        .or_insert_with(|| (c, Collision::default(), Collision::default()));
                    (&mut entry.1, &mut entry.2)
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
                        push(solid, *vertices, index(surface_of(material, log))?)?;
                        log.count("ground triangles", 1);
                    }
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
        for (n, (condition, solid, water)) in conditioned.into_values().enumerate() {
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
        };
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        assert_eq!(w.iter::<Condition>().count(), 0);
        // no water class chosen: one unconditioned ground soup, of the two files present (no empty water soup is made)
        assert_eq!(soups(&w).keys().cloned().collect::<Vec<_>>(), ["game".to_owned()]);
        let (_, game) = w.iter::<Collision>().next().unwrap();
        assert_eq!(game.triangles.len(), 2);
    }

    #[test]
    fn a_chosen_file_with_bodies_the_reader_could_not_decode_is_refused() {
        let mut w = world();
        let (b, _) = w.iter::<MapCollision>().find(|(_, m)| m.file == "b.hkx").unwrap();
        w.get_mut::<MapCollision>(b).unwrap().unread_bodies = vec!["hknpConvexShape".into()];
        let only_8 = LowerFromSoftCollision {
            hit_filters: HitFilters::new([8]).unwrap(),
            water: BTreeSet::new(),
            leave_out: BTreeSet::new(),
            reach: None,
        };
        only_8.run(&mut w.clone(), &mut PassLog::default()).unwrap();
        let with_15 = LowerFromSoftCollision {
            hit_filters: HitFilters::new([8, 15]).unwrap(),
            water: BTreeSet::new(),
            leave_out: BTreeSet::new(),
            reach: None,
        };
        let err = with_15.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(err.contains("b.hkx") && err.contains("hknpConvexShape"), "{err}");
    }
}
