//! FromSoft display groups lowered after preservation and before a core-only consumer.
//! The preserved NPC choice belongs to each occurrence, never to a shared clip.
//!
//! 711/713 intervals are lowered as [start, end). The game's exact end-instant behavior has
//! not been observed; half-open intervals avoid overlap between adjacent events. The old
//! preview included the end instant. 233 stays latched and OPEN_END never restores.
use std::collections::{BTreeMap, BTreeSet};
use primase_core::{Channel, Clip, ClipEvents, ClipTarget, Component, DrawGroupDefaults,
    DrawGroups, Entity, Instance, Interpolation, Lods, Property, World};
use primase_core::pass::{Pass, PassLog, PassSpec};
use crate::components::visibility::{ClipVisibility, PrimitiveDisplayMasks};

#[derive(Debug)]
pub struct LowerFromSoftDisplayMasks;
pub const HALF_OPEN_INFERENCE: &str = "inferred half-open [start,end) for TAE 711/713; end instant unobserved, adjacent intervals do not overlap";
pub const LOWER_FROMSOFT_DISPLAY_MASKS: PassSpec = PassSpec {
    name: "lower-fromsoft-display-masks", options: &[],
    make: |_| Ok(Box::new(LowerFromSoftDisplayMasks)),
};

fn timeline(source: &ClipVisibility, duration: f32) -> Result<Vec<Channel>, String> {
    if !source.refusals.is_empty() { return Err(source.refusals.join("; ")); }
    if source.character.trim().is_empty() || !duration.is_finite() || duration < 0. {
        return Err("invalid visibility scope or clip duration".into());
    }
    let mut touched = 0u32;
    let mut times = vec![0.];
    for change in &source.changes {
        if !change.start.is_finite() || change.start < 0. || change.end.is_nan()
            || change.end < change.start || change.show & change.hide != 0 {
            return Err("invalid display-mask interval or conflicting bits".into());
        }
        touched |= change.show | change.hide;
        if change.start <= duration { times.push(change.start); }
        if !change.latched && change.end < crate::tae::OPEN_END && change.end <= duration {
            times.push(change.end);
        }
    }
    times.sort_by(f32::total_cmp); times.dedup();
    let mut channels = Vec::new();
    for group in 0..32 {
        let bit = 1u32 << group;
        if touched & bit == 0 { continue; }
        let mut keys = Vec::new(); let mut values = Vec::new();
        for &time in &times {
            let mut value = -1.;
            for change in &source.changes {
                if time >= change.start && (change.latched || change.end >= crate::tae::OPEN_END || time < change.end) {
                    if change.hide & bit != 0 { value = 0.; }
                    if change.show & bit != 0 { value = 1.; }
                }
            }
            if values.last() != Some(&value) { keys.push(time); values.push(value); }
        }
        channels.push(Channel { target: ClipTarget::DrawGroup { scope: source.character.clone(), group },
            property: Property::Visibility, interpolation: Interpolation::Step, times: keys, values });
    }
    Ok(channels)
}

// A legacy default only states bits represented by this mesh. Combine that knowledge,
// refusing contradictory statements instead of letting the last LOD/part win.
fn merge_default(into: &mut BTreeMap<Entity, (String, u32, u32)>, entity: Entity,
    mask: &PrimitiveDisplayMasks) -> Result<(), String> {
    let covered = mask.groups.iter().flatten().fold(0u32, |bits, group| bits | (1u32 << group));
    let entry = into.entry(entity).or_insert((mask.character.clone(), 0, 0));
    if entry.0 != mask.character || (entry.2 ^ mask.default) & entry.1 & covered != 0 {
        return Err(format!("{entity:?}: conflicting occurrence display-mask defaults"));
    }
    entry.1 |= covered; entry.2 |= mask.default & covered;
    Ok(())
}

impl Pass for LowerFromSoftDisplayMasks {
    fn name(&self) -> &'static str { "lower-fromsoft-display-masks" }
    // Mesh-only reads and clip-only decode staging are both legitimate inputs; neither
    // optional source component is a mandatory prerequisite of the other branch.
    fn requires(&self) -> &[&'static str] { &[] }
    fn provides(&self) -> &[&'static str] { &[DrawGroups::NAME, DrawGroupDefaults::NAME, Clip::NAME, ClipEvents::NAME] }
    fn modifies(&self) -> &[&'static str] { &[DrawGroups::NAME, Clip::NAME, ClipEvents::NAME] }
    fn approximates(&self) -> &'static str { HALF_OPEN_INFERENCE }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut groups = Vec::new(); let mut defaults = BTreeMap::new();
        let forms: BTreeSet<_> = world.iter::<Lods>().flat_map(|(_,lods)|lods.forms.iter().map(|form|form.form)).collect();
        for (mesh, mask) in world.iter::<PrimitiveDisplayMasks>() {
            mask.verify(mesh, world)?;
            groups.push((mesh, DrawGroups { scope: mask.character.clone(),
                groups: mask.groups.iter().map(|group|group.map(u32::from)).collect() }));
            let mut placed = false;
            for (entity, instance) in world.iter::<Instance>() {
                if instance.prototype == mesh || world.get::<Lods>(instance.prototype)
                    .is_some_and(|lods|lods.forms.iter().any(|form|form.form == mesh)) {
                    merge_default(&mut defaults, entity, mask)?; placed = true;
                }
            }
            if !placed && !forms.contains(&mesh) { merge_default(&mut defaults, mesh, mask)?; }
        }
        let mut clips = Vec::new();
        for (entity, source) in world.iter::<ClipVisibility>() {
            let Some(original) = world.get::<Clip>(entity) else { continue; };
            let mut clip = original.clone();
            clip.channels.retain(|channel|!matches!(&channel.target,ClipTarget::DrawGroup {scope,..} if scope == &source.character));
            let mut events = world.get::<ClipEvents>(entity).cloned().unwrap_or_default();
            events.refusals.retain(|reason|!reason.starts_with("fromsoft.display_masks: "));
            match timeline(source, clip.duration).and_then(|channels|
                if clip.additive.is_some() && !channels.is_empty() {Err("additive draw-group channels are not defined".into())} else {Ok(channels)}) {
                Ok(channels) => {
                    let intervals=source.changes.iter().filter(|change|!change.latched && change.end < crate::tae::OPEN_END
                        && change.start<=clip.duration && change.start<change.end && (change.show|change.hide)!=0).count();
                    if intervals>0 {log.approximate(HALF_OPEN_INFERENCE,intervals as u64);}
                    clip.channels.extend(channels);
                }
                Err(reason) => events.refusals.push(format!("fromsoft.display_masks: {reason}; static defaults retained")),
            }
            clip.verify(entity, world)?;
            clips.push((entity, clip, events));
        }
        // Validate existing occurrence defaults before committing any branch of the plan.
        for (entity,(scope,_,_)) in &defaults {
            if world.get::<DrawGroupDefaults>(*entity).is_some_and(|existing|&existing.scope != scope) {
                return Err(format!("{entity:?}: incompatible draw-group default scope"));
            }
        }
        for (entity, groups) in groups { log.count("core draw-group meshes", 1); world.insert(entity, groups); }
        for (entity, (scope,_,shown)) in defaults {
            // Explicit occurrence defaults (two NPCs sharing a prototype) take precedence.
            if !world.has::<DrawGroupDefaults>(entity) { world.insert(entity, DrawGroupDefaults {scope,shown:u64::from(shown)}); }
        }
        for (entity, clip, events) in clips {
            for reason in &events.refusals {log.omit(reason,1);}
            log.count("core visibility channels",clip.channels.iter().filter(|channel|matches!(channel.target,ClipTarget::DrawGroup {..})).count() as u64);
            world.insert(entity, clip); world.insert(entity, events);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::visibility::MaskChange;
    fn run(world:&mut World)->primase_core::pass::RunReport {
        use primase_core::pass::{PassManager,PassRegistry,Pipeline,RunReport};
        let mut report=RunReport::default();
        PassManager::new(PassRegistry::new()).with_pass_checks(true).run(world,
            &Pipeline {passes:vec![Box::new(LowerFromSoftDisplayMasks)]},&mut report).unwrap();
        report
    }
    fn source() -> ClipVisibility { ClipVisibility {character:"c2120".into(),refusals:vec![],changes:vec![
        MaskChange {kind:713,start:1.,end:2.,show:1<<3,hide:0,latched:false},
        MaskChange {kind:233,start:3.,end:3.5,show:0,hide:1<<3,latched:true},
    ]} }
    fn sample(channel: &Channel,time:f32)->f32 {
        channel.times.iter().zip(&channel.values).rev().find(|(key,_)|**key<=time).map_or(-1.,|(_,v)|*v)
    }
    #[test] fn interval_end_inherits_latched_events_and_open_ends_do_not() {
        let channels=timeline(&source(),5.).unwrap();let channel=&channels[0];
        assert_eq!(sample(channel,0.9),-1.);assert_eq!(sample(channel,1.),1.);
        assert_eq!(sample(channel,2.0_f32.next_down()),1.);assert_eq!(sample(channel,2.),-1.);
        assert_eq!(sample(channel,4.),0.);
        let mut open=source();open.changes.truncate(1);open.changes[0].end=crate::tae::OPEN_END;
        assert_eq!(sample(&timeline(&open,200.).unwrap()[0],150.),1.);
    }
    #[test] fn source_order_overrides_and_a_refusal_disables_the_entire_timeline() {
        let mut source=source();source.changes.push(MaskChange {kind:711,start:1.5,end:2.5,show:0,hide:1<<3,latched:false});
        let channels=timeline(&source,5.).unwrap();assert_eq!(sample(&channels[0],1.75),0.);
        assert_eq!(sample(&channels[0],2.5),-1.);
        source.refusals.push("unknown mask byte".into());assert!(timeline(&source,5.).is_err());
    }
    #[test] fn two_occurrences_keep_their_defaults_and_mesh_remapping_is_generic() {
        use primase_core::{Mesh,Primitive,Topology};
        let mut world=World::new();let mesh=world.spawn();
        world.insert(mesh,Mesh::new(vec![Primitive {topology:Topology::Triangles,streams:std::collections::BTreeMap::from([
            (primase_core::Semantic::Position,primase_core::Stream::F32x3(vec![[0.,0.,0.],[1.,0.,0.],[0.,1.,0.]]))]),indices:None,material:None}]));
        world.insert(mesh,PrimitiveDisplayMasks {character:"c2120".into(),groups:vec![Some(3)],default:0});
        let a=world.spawn();let b=world.spawn();
        for entity in [a,b] {
            world.insert(entity,Instance {prototype:mesh,variant:0});
            world.insert(entity,primase_core::Transform::IDENTITY);
        }
        world.insert(b,DrawGroupDefaults {scope:"c2120".into(),shown:1<<3});
        run(&mut world);
        assert_eq!(world.get::<DrawGroupDefaults>(a).unwrap().shown,0);
        assert_eq!(world.get::<DrawGroupDefaults>(b).unwrap().shown,1<<3);
        assert_eq!(world.get::<DrawGroups>(mesh).unwrap().remap_primitives(&[0,0]).unwrap().unwrap().groups,vec![Some(3),Some(3)]);
    }
    #[test] fn malformed_visibility_keeps_other_channels_and_records_a_core_refusal() {
        let mut world=World::new();let entity=world.spawn();let mut source=source();source.refusals.push("unknown mask byte".into());
        let other=Channel {target:ClipTarget::Control {name:"unrelated".into()},property:Property::Scalar,
            interpolation:Interpolation::Step,times:vec![0.],values:vec![0.5]};
        world.insert(entity,Clip {name:"clip".into(),duration:5.,channels:vec![other.clone()],extrapolation:primase_core::Extrapolation::Hold,root_motion:None,additive:None});
        world.insert(entity,source);
        run(&mut world);
        assert_eq!(world.get::<Clip>(entity).unwrap().channels,vec![other]);
        assert!(world.get::<ClipEvents>(entity).unwrap().refusals[0].contains("static defaults retained"));
    }
    #[test] fn lowered_clip_evaluates_against_each_occurrence_and_only_its_own_scope() {
        let mut world=World::new();let skeleton=world.spawn();let entity=world.spawn();
        world.insert(skeleton,primase_core::Skeleton {joints:vec![]});
        world.insert(entity,Clip {name:"clip".into(),duration:5.,channels:vec![],extrapolation:primase_core::Extrapolation::Hold,root_motion:None,additive:None});
        world.insert(entity,source());
        let report=run(&mut world);
        assert_eq!(report.steps[0].log.approximations[HALF_OPEN_INFERENCE],1);
        let groups=DrawGroups {scope:"c2120".into(),groups:vec![Some(3),None]};
        let hidden=DrawGroupDefaults {scope:groups.scope.clone(),shown:0};
        let visible=DrawGroupDefaults {scope:groups.scope.clone(),shown:1<<3};
        let evaluate=|time|primase_animation::pose::evaluate(world.get::<Clip>(entity).unwrap(),skeleton,
            world.get::<primase_core::Skeleton>(skeleton).unwrap(),None,time).unwrap();
        let before=evaluate(0.);let during=evaluate(1.5);let end=evaluate(2.);
        assert!(!primase_animation::pose::shown(&hidden,&groups,&before,0).unwrap());
        assert!(primase_animation::pose::shown(&visible,&groups,&before,0).unwrap());
        assert!(primase_animation::pose::shown(&hidden,&groups,&during,0).unwrap());
        assert!(!primase_animation::pose::shown(&hidden,&groups,&end,0).unwrap());
        assert!(primase_animation::pose::shown(&hidden,&groups,&end,1).unwrap());
        let other=DrawGroups {scope:"another-character".into(),groups:groups.groups.clone()};
        let other_default=DrawGroupDefaults {scope:other.scope.clone(),shown:0};
        assert!(!primase_animation::pose::shown(&other_default,&other,&during,0).unwrap());
    }
}
