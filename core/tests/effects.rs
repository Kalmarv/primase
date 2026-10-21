//! A pass's effects are checked entity by entity against its declarations, not only by the names that appear or
//! disappear across the world: adding a name to a second entity, taking it from one of several, despawning, and
//! writing a resource (also by removing and inserting it again) are each refused undeclared and accepted declared; copies carry their source's names; in-place
//! edits of a value are declared ([`Pass::modifies`]) and not checked.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Component, Entity, Resource, World};

#[derive(Clone, Debug, PartialEq)]
struct X(u32);
impl Component for X {
    const NAME: &'static str = "toy.x";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

#[derive(Clone, Debug, PartialEq)]
struct Y;
impl Component for Y {
    const NAME: &'static str = "toy.y";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

#[derive(Clone, Debug, PartialEq)]
struct Setting(u32);
impl Resource for Setting {
    const NAME: &'static str = "toy.setting";
}

/// What the toy pass does to the world: e0 and e1 hold X, e0 holds Y too, the resource is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    AddToSecond,
    TakeFromOne,
    Despawn,
    Duplicate,
    CopyOnto,
    WriteResource,
    ReplaceResource,
    LendResource,
    EditInPlace,
}

struct Toy {
    act: Act,
    declared: bool,
}

impl Pass for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }
    fn provides(&self) -> &[&'static str] {
        if self.declared && self.act == Act::AddToSecond {
            &["toy.x"]
        } else {
            &[]
        }
    }
    fn removes(&self) -> &[&'static str] {
        if self.declared && self.act == Act::TakeFromOne {
            &["toy.x"]
        } else {
            &[]
        }
    }
    fn modifies(&self) -> &[&'static str] {
        if self.declared && matches!(self.act, Act::WriteResource | Act::ReplaceResource) {
            &["toy.setting"]
        } else {
            &[]
        }
    }
    fn despawns_entities(&self) -> bool {
        self.declared && self.act == Act::Despawn
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
        let (e0, e1, e2) = (
            Entity::from_index(0),
            Entity::from_index(1),
            Entity::from_index(2),
        );
        match self.act {
            Act::AddToSecond => {
                w.insert(e2, X(9));
            }
            Act::TakeFromOne => {
                w.remove::<X>(e1);
            }
            Act::Despawn => {
                w.despawn(e1);
            }
            Act::Duplicate => {
                w.duplicate(e0).ok_or("e0")?;
            }
            Act::CopyOnto => {
                w.copy_components(e0, e2);
            }
            Act::WriteResource => {
                w.resource_mut::<Setting>().ok_or("no setting")?.0 = 2;
            }
            // taken away and put back: a write, as much as resource_mut
            Act::ReplaceResource => {
                w.remove_resource::<Setting>().ok_or("no setting")?;
                w.insert_resource(Setting(3));
            }
            // lent read-only while the world changes, then put back: a borrow
            Act::LendResource => {
                let seen = w.resource_scope::<Setting, _>(|_, s| s.map(|s| s.0));
                if seen != Some(1) {
                    return Err(format!("lent {seen:?}"));
                }
            }
            Act::EditInPlace => {
                w.get_mut::<X>(e0).ok_or("e0")?.0 = 5;
            }
        }
        Ok(())
    }
}

fn world() -> World {
    let mut w = World::new();
    let (e0, e1, e2) = (w.spawn(), w.spawn(), w.spawn());
    w.insert(e0, X(1));
    w.insert(e0, Y);
    w.insert(e1, X(2));
    // e2 holds something, so it is content
    w.insert(e2, Y);
    w.insert_resource(Setting(1));
    w
}

fn run(act: Act, declared: bool) -> Result<RunReport, String> {
    // the checks under test, whatever the build
    let m = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let mut w = world();
    let mut report = RunReport::default();
    m.run(
        &mut w,
        &Pipeline {
            passes: vec![Box::new(Toy { act, declared })],
        },
        &mut report,
    )
    .map(|()| report)
    .map_err(|e| e.to_string())
}

#[test]
fn effects_on_entities_and_resources_are_checked_against_declarations() {
    // each effect a name-set comparison can't see is refused undeclared, naming it, and accepted declared
    for (act, words) in [
        (Act::AddToSecond, "added to entities"),
        (Act::TakeFromOne, "taken from entities"),
        (Act::Despawn, "despawned 1 entities"),
        (Act::WriteResource, "toy.setting"),
        (Act::ReplaceResource, "toy.setting"),
    ] {
        let err = run(act, false).unwrap_err();
        assert!(err.contains(words), "{act:?}: {err}");
        run(act, true).unwrap_or_else(|e| panic!("{act:?} declared: {e}"));
    }
    // copies carry their source's names: nothing to declare
    run(Act::Duplicate, false).unwrap();
    run(Act::CopyOnto, false).unwrap();
    // a resource lent with resource_scope is borrowed, not written: what the lender does is checked as usual
    run(Act::LendResource, false).unwrap();
    // an in-place edit of a value is not seen (declared in modifies, not checked: documented)
    run(Act::EditInPlace, false).unwrap();
}
