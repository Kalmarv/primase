//! Explicit far-view composition. Source tags distinguish real LongRange objects from merged stand-ins. A merged
//! cell overlapping the near rectangle is omitted whole: its orchestrator's per-object span hiding is unknown.
use crate::components::{
    AnvilBounds, AnvilFake, AnvilFarClass, AnvilFarSelected, AnvilImpostor, AnvilListed, AnvilLods,
};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Instance, Transform, World};

#[derive(Clone, Debug)]
pub struct SelectAnvilFarView {
    pub rect: [f64; 4],
    pub radius: f64,
}
pub const SELECT_ANVIL_FAR_VIEW: PassSpec = PassSpec {
    name: "select-anvil-far-view",
    options: &[
        OptionSpec::required(
            "rect",
            "-64,-64,64,64",
            "near rectangle in the source's ground axes",
        ),
        OptionSpec::required(
            "radius",
            "1000",
            "far-view distance beyond the near rectangle, metres",
        ),
    ],
    make: |g: &Given| {
        let rect: Vec<f64> = g
            .required("rect")
            .split(',')
            .map(|v| v.parse().map_err(|_| "rect must contain numbers"))
            .collect::<Result<_, _>>()?;
        let rect: [f64; 4] = rect.try_into().map_err(|_| "rect must contain four numbers")?;
        let radius: f64 = g
            .required("radius")
            .parse()
            .map_err(|_| "radius must be a number")?;
        if !rect.iter().all(|v| v.is_finite())
            || rect[0] >= rect[2]
            || rect[1] >= rect[3]
            || !radius.is_finite()
            || radius <= 0.0
        {
            return Err("invalid far-view rectangle/radius".into());
        }
        Ok(Box::new(SelectAnvilFarView { rect, radius }))
    },
};
impl Pass for SelectAnvilFarView {
    fn name(&self) -> &'static str {
        "select-anvil-far-view"
    }
    fn options(&self) -> Options {
        Options::from([
            ("rect".into(), self.rect.map(|v| v.to_string()).join(",")),
            ("radius".into(), self.radius.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilListed::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[AnvilFarClass::NAME, AnvilFarSelected::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[primase_core::Asset::NAME, primase_core::ClipBinding::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "far bounds from authored owner boxes; merged proxy cells overlapping the near rectangle omitted whole (per-object span hiding unread)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let ids: Vec<_> = world
            .iter::<AnvilListed>()
            .filter(|(_, listed)| {
                listed
                    .cells
                    .iter()
                    .any(|cell| cell.grid.rsplit(" - ").next() == Some("LongRange"))
                    || listed.reached_from >= 7
            })
            .map(|(e, l)| (e, l.clone()))
            .collect();
        let far = [
            self.rect[0] - self.radius,
            self.rect[1] - self.radius,
            self.rect[2] + self.radius,
            self.rect[3] + self.radius,
        ];
        for (entity, listed) in ids {
            let prototype = world.get::<Instance>(entity).map_or(entity, |i| i.prototype);
            let class = if world.has::<AnvilFake>(entity) {
                AnvilFarClass::MergedProxy
            } else if world.has::<AnvilImpostor>(prototype)
                || world
                    .get::<AnvilLods>(prototype)
                    .is_some_and(|l| l.forms.iter().any(|(_, f)| world.has::<AnvilImpostor>(*f)))
            {
                AnvilFarClass::Tree
            } else {
                world
                    .get::<AnvilFarClass>(prototype)
                    .copied()
                    .unwrap_or(AnvilFarClass::Real)
            };
            let bounds = world
                .get::<AnvilBounds>(entity)
                .map(|b| {
                    let a = listed.frame.point_to_source(b.min);
                    let b = listed.frame.point_to_source(b.max);
                    [
                        f64::from(a[0].min(b[0])),
                        f64::from(a[1].min(b[1])),
                        f64::from(a[0].max(b[0])),
                        f64::from(a[1].max(b[1])),
                    ]
                })
                .or_else(|| {
                    world.get::<Transform>(entity).map(|t| {
                        let p = listed.frame.point_to_source(t.cols[3]);
                        [f64::from(p[0]), f64::from(p[1]), f64::from(p[0]), f64::from(p[1])]
                    })
                });
            let meets = |rect: [f64; 4]| {
                bounds
                    .is_none_or(|b| b[0] <= rect[2] && b[2] >= rect[0] && b[1] <= rect[3] && b[3] >= rect[1])
            };
            let position = world
                .get::<Transform>(entity)
                .map(|t| listed.frame.point_to_source(t.cols[3]));
            let distance = position.map_or(0.0, |p| {
                let x = f64::from(p[0]);
                let y = f64::from(p[1]);
                let dx = (self.rect[0] - x).max(x - self.rect[2]).max(0.0);
                let dy = (self.rect[1] - y).max(y - self.rect[3]).max(0.0);
                dx.hypot(dy)
            });
            let in_authored_band = match class {
                AnvilFarClass::Canopy => (2500.0..=15000.0).contains(&distance) && distance <= self.radius,
                AnvilFarClass::Tree => (640.0..=3500.0).contains(&distance) && distance <= self.radius,
                _ => true,
            };
            let keep =
                meets(far) && in_authored_band && !(class == AnvilFarClass::MergedProxy && meets(self.rect));
            if keep {
                world.insert(entity, class);
                world.insert(entity, AnvilFarSelected);
                log.count(&format!("far-view {class:?} placements kept"), 1);
            } else {
                primase_core::ledger::despawn(
                    world,
                    entity,
                    log,
                    "far-view outside radius or merged proxy overlapping the near rectangle",
                );
                log.count(&format!("far-view {class:?} placements dropped"), 1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::ListedBy;
    use primase_core::{Entity, frame::SourceFrame};

    fn placed(world: &mut World, x: f32, class: AnvilFarClass) -> Entity {
        let entity = world.spawn();
        let mut transform = Transform::IDENTITY;
        transform.cols[3] = SourceFrame::ENU.point([x, 0.0, 0.0]);
        world.insert(entity, transform);
        world.insert(entity, class);
        world.insert(
            entity,
            AnvilListed {
                owner: u64::from(entity.index()),
                frame: SourceFrame::ENU,
                cells: vec![ListedBy {
                    grid: "USNAT - LongRange".into(),
                    cell: 1,
                    level: 0,
                }],
                reached_from: 0,
            },
        );
        if class == AnvilFarClass::MergedProxy {
            world.insert(
                entity,
                AnvilFake {
                    mesh_index: 1,
                    near: false,
                },
            );
        }
        entity
    }

    #[test]
    fn real_longrange_content_survives_while_overlapping_merged_cells_and_near_canopies_do_not() {
        let mut world = World::new();
        let real = placed(&mut world, 800.0, AnvilFarClass::Real);
        let merged_overlap = placed(&mut world, 0.0, AnvilFarClass::MergedProxy);
        let merged_far = placed(&mut world, 900.0, AnvilFarClass::MergedProxy);
        let canopy_near = placed(&mut world, 800.0, AnvilFarClass::Canopy);
        let canopy_far = placed(&mut world, 2800.0, AnvilFarClass::Canopy);
        SelectAnvilFarView {
            rect: [-64.0, -64.0, 64.0, 64.0],
            radius: 3000.0,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        assert!(!world.is_alive(merged_overlap));
        assert!(!world.is_alive(canopy_near));
        for entity in [real, merged_far, canopy_far] {
            assert!(world.has::<AnvilFarSelected>(entity));
        }
        super::super::SelectAnvilListed {
            coarse: super::super::Coarse::Overlap([-64.0, -64.0, 64.0, 64.0]),
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        assert!(world.is_alive(real));
        assert!(world.is_alive(merged_far));
        assert!(world.is_alive(canopy_far));
        assert!(world.iter::<AnvilFarSelected>().next().is_none());
    }
}
