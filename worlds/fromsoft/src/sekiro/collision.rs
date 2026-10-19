//! Map collision as the area stores it, and the pass that lowers it. The reader keeps, whole, every file of the
//! area's high-resolution collision binder that MSB collision parts name (`fromsoft.map_collision`); which parts are
//! ground, what each HitMtrl class becomes and how far around the read region to keep are `lower-fromsoft-collision`'s
//! options. Raw collision is never roof-smoothed here.
use super::settings::{HitClass, HitFilters, Metres, joined};
use super::source::{CollisionPart, CollisionSource, MapCollision, ReadScope};
use crate::{
    Archive, Binder, Result, collision,
    dcx::Decoder,
    invalid,
    msb::{MapLayout, PartKind},
};
use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::state::{Clause, Condition};
use primase_core::{
    Collision, Component, Entity, Name, Surfaces, Transform, World,
    frame::{Axis, SourceFrame},
    world::Resource,
};
use primase_havok::Compendium;
use std::collections::{BTreeMap, BTreeSet};
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// What the reader found of the area's map collision.
#[derive(Debug, Default)]
pub struct Report {
    /// Files read: those some MSB collision part names.
    pub files: usize,
    /// MSB collision parts by hit filter.
    pub parts_by_filter: BTreeMap<u8, usize>,
    /// Files of the binder no MSB collision part names: not read.
    pub unnamed_files: usize,
    pub triangles: usize,
    /// Triangles whose shape key binds no HitMtrl.
    pub unbound_triangles: usize,
    /// Collision files the MSB's parts name that the area's binder does not hold.
    pub missing_files: Vec<String>,
    /// Bodies of the read files whose shape class has no decoder, by class: listed on their file, not read.
    pub unread_bodies: BTreeMap<String, usize>,
}

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

fn leaf(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".dcx")
        .to_ascii_lowercase()
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

/// Read every collision file the area's MSB collision parts name, of every hit filter, as `fromsoft.map_collision`
/// entities in file-name order; with `states`, each part's clause.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    mut states: Option<&mut super::states::States>,
    world: &mut World,
) -> Result<Report> {
    let mut report = Report::default();
    let mut named = BTreeMap::<String, Vec<CollisionPart>>::new();
    for part in layout.parts.iter().filter(|p| p.kind == PartKind::Collision) {
        let filter = part
            .hit_filter
            .ok_or_else(|| invalid("collision", part.index, "missing hit filter"))?;
        let model = part
            .model
            .and_then(|i| layout.models.get(i))
            .ok_or_else(|| invalid("collision", part.index, "missing collision model"))?;
        let suffix = model
            .name
            .strip_prefix('h')
            .ok_or_else(|| invalid("collision", part.index, "unexpected collision model name"))?;
        *report.parts_by_filter.entry(filter).or_default() += 1;
        named
            .entry(format!("h{}_{suffix}.hkx", &area[1..]))
            .or_default()
            .push(CollisionPart {
                name: part.name.clone(),
                hit_filter: filter,
                condition: states.as_deref_mut().and_then(|s| s.clause(part)),
            });
    }
    let base = format!("/map/{area}/h{}", &area[1..]);
    let binder = Binder::read_split(
        &archive.read(&format!("{base}.hkxbhd"), decoder)?,
        &archive.read(&format!("{base}.hkxbdt"), decoder)?,
        decoder,
    )?;
    let comp = binder
        .files
        .iter()
        .find(|f| {
            f.name
                .as_deref()
                .is_some_and(|n| leaf(n).ends_with(".compendium"))
        })
        .ok_or_else(|| invalid("collision", 0, "missing compendium"))?;
    let comp = Compendium::read(&decoder.decode_if_dcx(&comp.bytes)?)?;
    if layout.map_offsets.len() > 1 {
        return Err(invalid("collision", 0, "multiple map offsets"));
    }
    let transform = layout
        .map_offsets
        .first()
        .map(|o| o.transform())
        .transpose()?
        .unwrap_or(Transform::IDENTITY);
    let mut files = binder
        .files
        .iter()
        .filter(|f| f.name.as_deref().is_some_and(|n| leaf(n).ends_with(".hkx")))
        .collect::<Vec<_>>();
    files.sort_by_key(|f| leaf(f.name.as_deref().unwrap_or("")));
    for file in files {
        let name = leaf(file.name.as_deref().unwrap_or(""));
        let Some(parts) = named.remove(&name) else {
            report.unnamed_files += 1;
            continue;
        };
        let raw = collision::read_map(&decoder.decode_if_dcx(&file.bytes)?, Some(&comp))?;
        report.files += 1;
        report.triangles += raw.triangles.len();
        report.unbound_triangles += raw.unmapped;
        for shape in &raw.unread_bodies {
            *report.unread_bodies.entry(shape.clone()).or_default() += 1;
        }
        let e = world.spawn();
        world.insert(e, Name(name.clone()));
        world.insert(
            e,
            MapCollision {
                file: name,
                parts,
                triangles: raw
                    .triangles
                    .into_iter()
                    .map(|t| t.map(|v| super::point(&transform, FRAME.point(v))))
                    .collect(),
                materials: raw.materials,
                invalid_material_keys: raw.invalid_material_keys,
                dead_primitives: raw.dead_primitives,
                unread_bodies: raw.unread_bodies,
            },
        );
    }
    report.missing_files = named.into_keys().collect();
    Ok(report)
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
    Ok(scope.region.map(|r| (r, reach)))
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
/// Options: `hit-filters=` the filters whose parts are lowered, joined by `+` (required: which placed collision is
/// ground is the program's choice); `water=` HitMtrl classes lowered into separate `water` soups with the `default`
/// surface; `leave-out=` classes not lowered, counted; `reach=N` leaves out, counted, the triangles whose centre lies
/// more than N metres outside the read region (`fromsoft.read_scope`). Without these every triangle of the chosen
/// files is ground.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftCollision {
    /// The hit filters whose parts' files are lowered.
    pub hit_filters: HitFilters,
    /// What becomes of the triangles of a HitMtrl class other than ground; a class not listed is ground.
    pub classes: BTreeMap<HitClass, ClassChoice>,
    /// The crop around the read region, if any.
    pub reach: Option<Metres>,
}

/// What `lower-fromsoft-collision` makes of a HitMtrl class's triangles instead of ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassChoice {
    /// The `water` soups, with the `default` surface.
    Water,
    /// Not lowered, counted.
    LeaveOut,
}

impl LowerFromSoftCollision {
    /// Every triangle of the files that parts of `hit_filters` name lowered as ground, uncropped.
    pub fn new(hit_filters: HitFilters) -> Self {
        LowerFromSoftCollision {
            hit_filters,
            classes: BTreeMap::new(),
            reach: None,
        }
    }

    pub fn from_options(o: &Options) -> std::result::Result<Self, String> {
        let filters = o
            .get("hit-filters")
            .ok_or("hit-filters is required: which placed collision is ground is the program's choice")?;
        let mut pass = Self::new(HitFilters::parse("hit-filters", filters)?);
        for (k, v) in o {
            let choice = match k.as_str() {
                "hit-filters" => continue,
                "water" => ClassChoice::Water,
                "leave-out" => ClassChoice::LeaveOut,
                "reach" => {
                    pass.reach = Some(Metres::parse(k, v)?);
                    continue;
                }
                _ => return Err(format!("unknown option {k}")),
            };
            for class in HitClass::parse_set(k, v)? {
                if pass.classes.insert(class, choice).is_some() {
                    return Err(format!("HitMtrl class {class} is both water and left out"));
                }
            }
        }
        Ok(pass)
    }

    fn classes_of(&self, choice: ClassChoice) -> Vec<HitClass> {
        self.classes
            .iter()
            .filter(|(_, c)| **c == choice)
            .map(|(class, _)| *class)
            .collect()
    }
}

impl Pass for LowerFromSoftCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([("hit-filters".to_owned(), self.hit_filters.to_string())]);
        for (key, choice) in [
            ("water", ClassChoice::Water),
            ("leave-out", ClassChoice::LeaveOut),
        ] {
            let classes = self.classes_of(choice);
            if !classes.is_empty() {
                o.insert(key.into(), joined(classes));
            }
        }
        if let Some(r) = self.reach {
            o.insert("reach".into(), r.to_string());
        }
        o
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
            let (solid, water) = match super::states::States::any(&clauses) {
                None => (&mut solid, &mut water),
                Some(c) => {
                    let entry = conditioned
                        .entry(super::states::condition_key(&c))
                        .or_insert_with(|| (c, Collision::default(), Collision::default()));
                    (&mut entry.1, &mut entry.2)
                }
            };
            for (vertices, &material) in record.triangles.iter().zip(&record.materials) {
                if crop.is_some_and(|c| beyond(vertices, c)) {
                    log.count("triangles beyond the reach around the read region, left out", 1);
                    continue;
                }
                match HitClass::of(material).and_then(|c| self.classes.get(&c)) {
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
            region: Some([-10., -10., 10., 10.]),
            region_margin: 30.,
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
        let pass = LowerFromSoftCollision::from_options(&Options::from([
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
    fn without_policy_options_every_triangle_of_the_chosen_files_is_ground() {
        let mut w = world();
        let pass =
            LowerFromSoftCollision::from_options(&Options::from([("hit-filters".into(), "8+15".into())]))
                .unwrap();
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        assert_eq!(soups(&w)["game"].len(), 6);
        assert!(w.iter::<MapCollision>().next().is_none());
        // the filters are the program's; a class cannot be both water and left out
        assert!(LowerFromSoftCollision::from_options(&Options::new()).is_err());
        assert!(
            LowerFromSoftCollision::from_options(&Options::from([
                ("hit-filters".into(), "8".into()),
                ("water".into(), "22".into()),
                ("leave-out".into(), "22".into()),
            ]))
            .is_err()
        );
    }

    #[test]
    fn a_chosen_file_with_bodies_the_reader_could_not_decode_is_refused() {
        let mut w = world();
        let (b, _) = w.iter::<MapCollision>().find(|(_, m)| m.file == "b.hkx").unwrap();
        w.get_mut::<MapCollision>(b).unwrap().unread_bodies = vec!["hknpConvexShape".into()];
        let only_8 = LowerFromSoftCollision::new(HitFilters::new([8]).unwrap());
        only_8.run(&mut w.clone(), &mut PassLog::default()).unwrap();
        let with_15 = LowerFromSoftCollision::new(HitFilters::new([8, 15]).unwrap());
        let err = with_15.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(err.contains("b.hkx") && err.contains("hknpConvexShape"), "{err}");
    }
}
