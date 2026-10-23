//! `lower-fromsoft-extras`: the area extras the reader kept (`fromsoft.area_extras`) as core curves and markers.
use crate::sekiro::source::{AreaExtras, SpawnRotation};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Curve, Marker, Name, Resource, Source, Transform, World};

/// `lower-fromsoft-extras`: the area's extras (`fromsoft.area_extras`, taken out) as core records in source order:
/// each hang-edge segment a two-point `core.curve` of kind `ledge` (labelled `<edge file>:<n>` by its place in its
/// file), each spawn point a `core.marker` of kind `travel` at its position, named as stored, its stored rotation
/// kept as `fromsoft.spawn_point`. Nothing is joined, rounded, renamed or turned: chaining ledges, choosing a
/// spawn and naming markers for players are a program's.
#[derive(Clone, Copy, Debug)]
pub struct LowerFromSoftExtras;

/// `lower-fromsoft-extras`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_FROMSOFT_EXTRAS: PassSpec = PassSpec {
    name: "lower-fromsoft-extras",
    options: &[],
    make: |_| Ok(Box::new(LowerFromSoftExtras)),
};

impl Pass for LowerFromSoftExtras {
    fn name(&self) -> &'static str {
        "lower-fromsoft-extras"
    }
    fn requires(&self) -> &[&'static str] {
        &[AreaExtras::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Curve::NAME,
            Marker::NAME,
            Transform::NAME,
            Name::NAME,
            Source::NAME,
            SpawnRotation::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[AreaExtras::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let extras = world
            .remove_resource::<AreaExtras>()
            .ok_or("no fromsoft.area_extras")?;
        let mut in_file = std::collections::BTreeMap::<&str, usize>::new();
        for segment in &extras.hang_segments {
            let n = in_file.entry(&segment.file).or_default();
            let e = world.spawn();
            world.insert(
                e,
                Curve {
                    kind: "ledge".into(),
                    points: segment.points.to_vec(),
                    closed: false,
                    tangents: None,
                    up: None,
                },
            );
            world.insert(e, Source(format!("{}:{n}", segment.file)));
            *n += 1;
            log.place(e);
            log.count("ledge segments", 1);
        }
        for point in &extras.spawn_points {
            let e = world.spawn();
            world.insert(
                e,
                Marker {
                    kind: "travel".into(),
                    ground: None,
                    yaw: None,
                },
            );
            world.insert(
                e,
                Transform {
                    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], point.position],
                },
            );
            world.insert(e, Name(point.name.clone()));
            world.insert(
                e,
                SpawnRotation {
                    rotation_degrees: point.rotation_degrees,
                },
            );
            log.place(e);
            log.count("travel markers", 1);
        }
        Ok(())
    }
}
