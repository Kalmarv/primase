//! The Sekiro area reader in the convert loop: slice A of m11_01 read once, its namespaces only `core` and
//! `fromsoft`, every placed prototype with geometry boxed by its stored model's world box, and the whole world as one
//! part written with every placement the read kept.
//!
//! Needs `PRIMASE_SEKIRO_DIR` and `PRIMASE_OODLE6`.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use primase_core::convert::{Finished, Reader, Result, WholeWorld, Writer, check_emits, convert_parts};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, Target};
use primase_core::{Instance, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::rigid::SourceModel;
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::{self, world_bounds};
use primase_testdata::real_data;
mod common;

/// Every part's placements.
struct Placements(Rc<RefCell<Vec<usize>>>);
impl Writer for Placements {
    fn target(&self) -> Target {
        Target::new("placements").carry_unknown()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        self.0.borrow_mut().push(part.iter::<Instance>().count());
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

#[test]
fn slice_a_reads_through_the_convert_loop() {
    let (Some(root), Some(dll)) = (real_data("PRIMASE_SEKIRO_DIR"), real_data("PRIMASE_OODLE6")) else {
        return;
    };
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let options = sekiro::Options {
        region: Some(sekiro::Region {
            rect: [-170.0, 180.0, -50.0, 300.0],
            margin: 30.0,
        }),
        ..Default::default()
    };
    let mut reader = AreaReader::new(&root, "m11_01", decoder, options.clone());
    let (world, report) = reader.read().unwrap();
    check_emits(&world, reader.emits(), "the read").unwrap();
    assert!(report.iter().any(|(k, _)| k == "sekiro area"), "{report:?}");
    let placements = world.iter::<Instance>().count();
    assert!(placements > 0);
    // every placed prototype with geometry has a box, its model's world box
    let prototypes: BTreeSet<_> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    let mut boxed = 0;
    for &p in &prototypes {
        let model = &world.get::<SourceModel>(p).unwrap().0;
        let want = world_bounds(model).unwrap().map(|b| b.map(|c| c.map(f64::from)));
        assert_eq!(reader.prototype_box(&world, p), want);
        boxed += usize::from(want.is_some());
    }
    assert!(boxed > 0);
    // a part's load is its roots' closure
    let root_set: BTreeSet<_> = world.iter::<Instance>().map(|(e, _)| e).take(3).collect();
    let (part, _) = reader.load(&world, &root_set).unwrap();
    assert_eq!(part.iter::<Instance>().count(), root_set.len());
    // through the convert loop, the whole world as one part
    let written = Rc::new(RefCell::new(Vec::new()));
    convert_parts(
        &mut AreaReader::new(&root, "m11_01", decoder, options),
        &mut WholeWorld::default(),
        Box::new(Placements(written.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Pipeline { passes: Vec::new() },
    )
    .unwrap();
    assert_eq!(*written.borrow(), [placements]);
    println!(
        "slice A: {placements} placements, {} prototypes, {boxed} boxed",
        prototypes.len()
    );
}

/// The reader declares every name its read and load put in a world (`sekiro::convert::EMITTED_NAMES`, what a coverage
/// report lists): slice A, read and loaded whole, holds no other name, and every declared name.
#[test]
fn the_reader_declares_every_name_it_emits() {
    let (Some(root), Some(dll)) = (real_data("PRIMASE_SEKIRO_DIR"), real_data("PRIMASE_OODLE6")) else {
        return;
    };
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    // read without states and with the event-flag states (the only read that emits conditions and state axes)
    let mut seen = BTreeSet::new();
    for states in [sekiro::StateClasses::None, sekiro::StateClasses::Event] {
        let options = sekiro::Options {
            region: Some(sekiro::Region {
                rect: [-170.0, 180.0, -50.0, 300.0],
                margin: 30.0,
            }),
            states,
            ..Default::default()
        };
        let mut reader = AreaReader::new(&root, "m11_01", decoder, options);
        let (world, _) = reader.read().unwrap();
        let all: BTreeSet<_> = world.iter::<Instance>().map(|(e, _)| e).collect();
        let (loaded, _) = reader.load(&world, &all).unwrap();
        for w in [&world, &loaded] {
            for n in w.component_names().into_iter().chain(w.resource_names()) {
                seen.insert(n.to_owned());
            }
        }
    }
    let declared: BTreeSet<&str> = primase_fromsoft::sekiro::convert::EMITTED_NAMES
        .iter()
        .copied()
        .collect();
    let undeclared: Vec<&String> = seen.iter().filter(|n| !declared.contains(n.as_str())).collect();
    assert!(undeclared.is_empty(), "emitted but not declared: {undeclared:?}");
    let unseen: Vec<&&str> = declared.iter().filter(|n| !seen.contains(**n)).collect();
    assert!(
        unseen.is_empty(),
        "declared, emitted by slice A neither: {unseen:?}"
    );
}
