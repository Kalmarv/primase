//! `lower-gta-lights` through a conversion (`convert_parts`), not the pass alone: the lights it makes reach the writer,
//! whether it runs in the selection (before the parts are planned) or as a part pass (after the part's roots are
//! fixed: each light is its placement's content, `PassLog::place_from`).

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use primase_core::convert::{Finished, Reader, Report, Result, WholeWorld, Writer, convert_parts};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, Target};
use primase_core::{Entity, Instance, Light, Name, Transform, World};
use primase_gta::components::GtaLights;
use primase_gta::passes::lights::{LightHours, LowerGtaLights};
use primase_gta::rage::drawable::LightAttr;

/// One placement of a prototype holding one point light.
struct OneLamp(World);

impl OneLamp {
    fn new() -> OneLamp {
        let mut w = World::new();
        let proto = w.spawn();
        w.insert(proto, Name("lamp".into()));
        w.insert(
            proto,
            GtaLights {
                records: vec![LightAttr {
                    intensity: 1.0,
                    falloff: 10.0,
                    kind: 1,
                    direction: [0.0, 0.0, -1.0],
                    ..LightAttr::default()
                }],
            },
        );
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        OneLamp(w)
    }
}

impl Reader for OneLamp {
    fn read(&mut self) -> Result<(World, Report)> {
        let all: BTreeSet<Entity> = self.0.entities().collect();
        Ok((self.0.subset(&all), Report::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "gta"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// Counts the lights of every part it is given.
struct CountLights(Rc<RefCell<usize>>);

impl Writer for CountLights {
    fn target(&self) -> Target {
        Target::new("lights").carry_unknown()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        *self.0.borrow_mut() += part.iter::<Light>().count();
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Report::new().into())
    }
}

fn lights_written(in_selection: bool) -> usize {
    let mut registry = PassRegistry::new();
    primase_gta::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let pass = || -> Pipeline {
        Pipeline {
            passes: vec![Box::new(LowerGtaLights {
                hours: LightHours::Every,
                candela_per_unit: 1771.0,
                region: None,
            })],
        }
    };
    let empty = || Pipeline { passes: Vec::new() };
    let (select, passes) = if in_selection {
        (pass(), empty())
    } else {
        (empty(), pass())
    };
    let count = Rc::new(RefCell::new(0));
    convert_parts(
        &mut OneLamp::new(),
        &mut WholeWorld::default(),
        Box::new(CountLights(count.clone())),
        &manager,
        &select,
        &passes,
    )
    .unwrap();
    *count.borrow()
}

/// The light a placement's prototype holds reaches the writer as a part pass makes it (the part's roots were fixed
/// before it ran) and as the selection makes it.
#[test]
fn a_placements_light_reaches_the_writer_from_a_part_pass_and_from_the_selection() {
    assert_eq!(lights_written(false), 1, "lower-gta-lights as a part pass");
    assert_eq!(lights_written(true), 1, "lower-gta-lights in the selection");
}
