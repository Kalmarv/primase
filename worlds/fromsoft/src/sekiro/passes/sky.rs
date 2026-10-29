//! `lower-fromsoft-sky`: the sky the area read brought in ([`crate::sekiro::read::sky`], `fromsoft.sky_part`) lowered
//! to core's sky: the moon's `core.sky_anchor` (towards the sun, at the game's distance) and the draw order of the sky's
//! materials (core's sky layer, in the order the game draws them). The domes' and rings' anchors and every part's
//! states are the read's already. Elden Ring's weather assets put their sky programs' materials on the sky layer (see
//! [`LowerFromSoftSky`]).
use crate::materials::{Material, SkipClass};
use crate::sekiro::read::sky::MOON_DISTANCE;
use crate::sekiro::source::{MaterialSlots, SkyPart, SkyRole};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{
    Component, DrawLayer, DrawOrder, Entity, Environment, Instance, Light, LightKind, Lods, Mesh, SkyAnchor,
    Transform, World,
};
use std::collections::{BTreeMap, BTreeSet};

/// `lower-fromsoft-sky`: each moon placement gets `core.sky_anchor` towards the environment's sun (`core.environment`'s
/// `sun`, a directional light: `lower-fromsoft-environment` makes it from the area's light set, whose first directional
/// light is the one a recorded frame places the moon towards) at the game's 25 units; with no sun a moon has nothing to
/// stand towards and is despawned, counted. Every material the sky's prototypes draw with (their material slots and
/// the primitives of their meshes and level-of-detail forms; shadow proxies aside) gets `core.draw_order` on the sky
/// layer, writing depth as the game's do, queued in the order a recorded frame draws them: the dome
/// ([`DrawOrder::SKY`]), the moon (+1), the rings (+2). A material that a placement outside the sky also draws keeps
/// its own order, counted.
///
/// An Elden Ring weather asset's materials of a sky program (the material table's sky class: `M[Sky9]`, `M[Clouds5]`,
/// `M[Clouds9]`, `M[Atmosphere][Clouds][EnvmapCapture]`) go on the sky layer at [`DrawOrder::SKY`], writing depth, in
/// the source's order; its other materials (the Erdtree's light shafts) keep their own, counted. A recorded frame of
/// Stormveil draws its cloud layer (`AEG096_603`, `M[Clouds9]`) so: after the opaque surfaces are lit and composited
/// and before the blended surfaces, with a pipeline that tests and writes depth and blends by source alpha; that frame
/// composites its volumetric fog after the sky, over it (Sekiro's composites it before its sky). The sky spheres'
/// own draws are not yet observed: that they draw as the cloud layer does is inferred from the program family. Then
/// `fromsoft.sky_part` is removed. No options: what it lowers is the game's.
#[derive(Clone, Copy, Debug)]
pub struct LowerFromSoftSky;

/// `lower-fromsoft-sky`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LOWER_FROMSOFT_SKY: PassSpec = PassSpec {
    name: "lower-fromsoft-sky",
    options: &[],
    make: |_| Ok(Box::new(LowerFromSoftSky)),
};

/// Where a sky role's surfaces come in the sky layer and whether they write depth: Sekiro's in the order of a recorded
/// frame's draws (dome, moon, far mountains), writing depth as they do there; an Elden Ring weather asset's at the
/// layer's start, writing depth as the recorded cloud layer's pipeline does (see [`LowerFromSoftSky`]).
fn order(role: SkyRole) -> (i32, bool) {
    match role {
        SkyRole::Dome => (DrawOrder::SKY, true),
        SkyRole::Moon => (DrawOrder::SKY + 1, true),
        SkyRole::Ring => (DrawOrder::SKY + 2, true),
        SkyRole::Weather | SkyRole::WeatherSky => (DrawOrder::SKY, true),
    }
}

/// The material entities prototype `p` draws with: its source slots and the materials of its mesh's and its forms'
/// primitives.
fn materials_of(world: &World, p: Entity) -> BTreeSet<Entity> {
    let mut out = BTreeSet::new();
    if let Some(slots) = world.get::<MaterialSlots>(p) {
        out.extend(slots.0.iter().copied());
    }
    let forms = world
        .get::<Lods>(p)
        .map(|l| l.forms.iter().map(|f| f.form).collect::<Vec<_>>())
        .unwrap_or_default();
    for e in std::iter::once(p).chain(forms) {
        if let Some(mesh) = world.get::<Mesh>(e) {
            out.extend(mesh.primitives().iter().filter_map(|prim| prim.material));
        }
    }
    out
}

impl Pass for LowerFromSoftSky {
    fn name(&self) -> &'static str {
        "lower-fromsoft-sky"
    }
    fn requires(&self) -> &[&'static str] {
        &[SkyPart::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[SkyAnchor::NAME, DrawOrder::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[SkyPart::NAME]
    }
    /// A moon with no sun to stand towards.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let parts: Vec<(Entity, SkyPart)> = world.iter::<SkyPart>().map(|(e, p)| (e, *p)).collect();
        let sun = world
            .resource::<Environment>()
            .and_then(|env| env.sun)
            .filter(|&s| {
                world.has::<Transform>(s)
                    && world
                        .get::<Light>(s)
                        .is_some_and(|l| l.kind == LightKind::Directional)
            });
        // the materials each role of the sky draws with, and the prototypes the sky places
        let mut wanted = BTreeMap::<Entity, (i32, bool)>::new();
        let mut sky_prototypes = BTreeSet::new();
        for (e, part) in &parts {
            let Some(prototype) = world.get::<Instance>(*e).map(|i| i.prototype) else {
                return Err(format!(
                    "{e:?}: a sky part that is no placement (no core.instance)"
                ));
            };
            sky_prototypes.insert(prototype);
            let wants = order(part.role);
            for m in materials_of(world, prototype) {
                let skip = world.get::<Material>(m).and_then(|m| m.skip);
                if skip == Some(SkipClass::Shadow) {
                    continue;
                }
                if matches!(part.role, SkyRole::Weather | SkyRole::WeatherSky) && skip != Some(SkipClass::Sky)
                {
                    log.count(
                        "weather asset materials of no sky program, left in their own layer",
                        1,
                    );
                    continue;
                }
                // a material two roles share draws where the earlier one does
                let q = wanted.entry(m).or_insert(wants);
                *q = (*q).min(wants);
            }
        }
        // the materials the rest of the world draws with
        let others: BTreeSet<Entity> = world
            .iter::<Instance>()
            .filter(|(e, i)| !world.has::<SkyPart>(*e) && !sky_prototypes.contains(&i.prototype))
            .map(|(_, i)| i.prototype)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .flat_map(|p| materials_of(world, p))
            .collect();
        for (m, (queue, depth_write)) in wanted {
            if others.contains(&m) {
                log.count(
                    "sky materials other placements draw too, left in their own layer",
                    1,
                );
                continue;
            }
            world.insert(
                m,
                DrawOrder {
                    layer: DrawLayer::Sky,
                    queue,
                    depth_write,
                },
            );
            log.count("sky materials on the sky layer", 1);
        }
        for (e, part) in parts {
            world.remove::<SkyPart>(e);
            if part.role != SkyRole::Moon {
                continue;
            }
            match sun {
                Some(light) => {
                    world.insert(
                        e,
                        SkyAnchor::TowardsLight {
                            light,
                            distance: MOON_DISTANCE,
                        },
                    );
                    log.count("moons placed towards the sun", 1);
                }
                None => {
                    primase_core::ledger::despawn(
                        world,
                        e,
                        log,
                        "a moon with no sun to stand towards (core.environment names none)",
                    );
                    log.count("moons dropped: no sun", 1);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(w: &mut World, prototype: Entity, role: SkyRole) -> Entity {
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(e, SkyPart { role, states: 0 });
        e
    }

    fn prototype(w: &mut World, materials: &[Entity]) -> Entity {
        let p = w.spawn();
        w.insert(p, MaterialSlots(materials.to_vec()));
        p
    }

    fn sun(w: &mut World) -> Entity {
        let s = w.spawn();
        w.insert(
            s,
            Light {
                kind: LightKind::Directional,
                color: [1.; 3],
                intensity: 1.,
                range: None,
                cast_shadows: None,
                radius: None,
            },
        );
        w.insert(s, Transform::IDENTITY);
        w.insert_resource(Environment {
            sun: Some(s),
            ambient: None,
        });
        s
    }

    /// The dome's, the moon's and the ring's materials on the sky layer in the frame's order, a material a map piece
    /// also draws left alone, the moon towards the sun at 25, the source records gone.
    #[test]
    fn the_sky_draws_on_its_layer_and_the_moon_stands_towards_the_sun() {
        let mut w = World::new();
        let [dome_m, moon_m, ring_m, shared] = [(); 4].map(|_| w.spawn());
        let dome_p = prototype(&mut w, &[dome_m]);
        let moon_p = prototype(&mut w, &[moon_m]);
        let ring_p = prototype(&mut w, &[ring_m, shared]);
        let piece_p = prototype(&mut w, &[shared]);
        let dome = placement(&mut w, dome_p, SkyRole::Dome);
        let moon = placement(&mut w, moon_p, SkyRole::Moon);
        placement(&mut w, ring_p, SkyRole::Ring);
        let piece = w.spawn();
        w.insert(
            piece,
            Instance {
                prototype: piece_p,
                variant: 0,
            },
        );
        let light = sun(&mut w);
        let mut log = PassLog::default();
        LowerFromSoftSky.run(&mut w, &mut log).unwrap();
        for (m, q) in [(dome_m, 2900), (moon_m, 2901), (ring_m, 2902)] {
            assert_eq!(
                w.get::<DrawOrder>(m),
                Some(&DrawOrder {
                    layer: DrawLayer::Sky,
                    queue: q,
                    depth_write: true
                })
            );
        }
        assert!(w.get::<DrawOrder>(shared).is_none());
        assert_eq!(
            w.get::<SkyAnchor>(moon),
            Some(&SkyAnchor::TowardsLight {
                light,
                distance: 25.0
            })
        );
        assert!(
            w.get::<SkyAnchor>(dome).is_none(),
            "the read anchors domes, not this pass"
        );
        assert!(w.iter::<SkyPart>().next().is_none());
        assert_eq!(log.counts["sky materials on the sky layer"], 3);
    }

    /// A material of `shader` with the skip class `skip`, as the material table gives it.
    fn material(w: &mut World, shader: &str, skip: Option<SkipClass>) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            Material {
                mtd: format!("{shader}_material"),
                definition: None,
                gx: Vec::new(),
                shader: shader.into(),
                class: "opaque".into(),
                skip,
                cutoff: 0.5,
                bindings: BTreeMap::new(),
                layered: None,
            },
        );
        m
    }

    /// An Elden Ring weather asset's sky-program materials go on the sky layer at its start, writing depth; a material
    /// of another program (the Erdtree's light shafts) keeps its order, counted; the pass adds no anchor (the read
    /// anchors a sky-program asset); the record goes.
    #[test]
    fn a_weather_asset_draws_its_sky_programs_on_the_sky_layer() {
        let mut w = World::new();
        let sphere = material(&mut w, "M[Sky9]", Some(SkipClass::Sky));
        let clouds = material(&mut w, "M[Clouds5]", Some(SkipClass::Sky));
        let shaft = material(&mut w, "M[WorldTree_LightShaft]", Some(SkipClass::LightVolume));
        let sky_p = prototype(&mut w, &[sphere, clouds]);
        let tree_p = prototype(&mut w, &[shaft]);
        let asset = placement(&mut w, sky_p, SkyRole::WeatherSky);
        placement(&mut w, tree_p, SkyRole::Weather);
        let mut log = PassLog::default();
        LowerFromSoftSky.run(&mut w, &mut log).unwrap();
        for m in [sphere, clouds] {
            assert_eq!(
                w.get::<DrawOrder>(m),
                Some(&DrawOrder {
                    layer: DrawLayer::Sky,
                    queue: DrawOrder::SKY,
                    depth_write: true
                })
            );
        }
        assert!(w.get::<DrawOrder>(shaft).is_none());
        assert!(w.get::<SkyAnchor>(asset).is_none() && w.get::<SkyPart>(asset).is_none());
        assert!(w.is_alive(asset));
        assert_eq!(
            log.counts["weather asset materials of no sky program, left in their own layer"],
            1
        );
        assert_eq!(log.counts["sky materials on the sky layer"], 2);
    }

    /// Without a sun the moon has nowhere to stand: it is despawned, counted; the materials still go on the layer.
    #[test]
    fn a_moon_without_a_sun_is_dropped() {
        let mut w = World::new();
        let m = w.spawn();
        let p = prototype(&mut w, &[m]);
        let moon = placement(&mut w, p, SkyRole::Moon);
        let mut log = PassLog::default();
        LowerFromSoftSky.run(&mut w, &mut log).unwrap();
        assert!(!w.is_alive(moon));
        assert_eq!(log.counts["moons dropped: no sun"], 1);
        assert_eq!(w.get::<DrawOrder>(m).map(|d| d.queue), Some(2901));
    }
}
