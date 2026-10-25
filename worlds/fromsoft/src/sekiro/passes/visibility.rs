//! Keep display-mask candidates for playback instead of permanently dropping the selected NPC's hidden meshes.
//! Explicit opt-in before resolve-state; other world-state conditions retain their existing resolution rules.
use primase_core::{Component, World};
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::{PrimitiveCondition, StateAssignment, StateAxis};
use crate::tae::visibility::PrimitiveDisplayMasks;

#[derive(Debug)]
pub struct PreserveDisplayMasks { show_unassigned:bool }
impl Pass for PreserveDisplayMasks {
    fn name(&self)->&'static str {"preserve-fromsoft-display-masks"}
    fn options(&self)->Options {Options::from([("defaults".into(),if self.show_unassigned {"show"}else{"assigned"}.into())])}
    fn provides(&self)->&[&'static str] {&[PrimitiveDisplayMasks::NAME]}
    fn requires(&self)->&[&'static str] {&[PrimitiveCondition::NAME,StateAxis::NAME]}
    // This opt-in playback representation is not a general legalization of arbitrary world state.
    fn legalizes(&self)->&[&'static str] {&[]}
    fn removes(&self)->&[&'static str] {&[PrimitiveCondition::NAME]}
    fn run(&self,world:&mut World,log:&mut PassLog)->Result<(),String> {
        let assignment=world.resource::<StateAssignment>();
        let mut plans=Vec::new();
        for (entity,conditions) in world.iter::<PrimitiveCondition>() {
            let mut character=None;let mut groups=Vec::new();let mut default=0;
            let mut has_mask=false;
            // First identify mask-owned conditions; an unrelated stateful mesh is left to resolve-state.
            for literal in conditions.0.iter().flatten().flat_map(|c|&c.0).flat_map(|c|&c.0) {
                if world.get::<StateAxis>(literal.axis).is_some_and(|a|a.class==super::outfits::MASK_CLASS) {has_mask=true;}
            }
            if !has_mask {continue;}
            for condition in &conditions.0 {
                let Some(condition)=condition else {groups.push(None);continue;};
                let [clause]=condition.0.as_slice() else {return Err(format!("{entity:?}: compound display mask condition is not a single group"));};
                let [literal]=clause.0.as_slice() else {return Err(format!("{entity:?}: display mask mixed with other state; cannot move it into playback"));};
                let axis=world.get::<StateAxis>(literal.axis).ok_or("display mask axis absent")?;
                if axis.class!=super::outfits::MASK_CLASS || literal.values!=0b10 {return Err(format!("{entity:?}: not a positive display mask condition"));}
                let (chr,group)=axis.key.strip_prefix("fromsoft.mask.").and_then(|s|s.rsplit_once('.'))
                    .ok_or("display mask axis identity is not fromsoft.mask.<character>.<group>")?;
                let group=group.parse::<u8>().map_err(|e|e.to_string())?;
                if group>=32 {return Err("display mask group outside 0..31".into());}
                if character.as_ref().is_some_and(|c|c!=chr) {return Err(format!("{entity:?}: mixed character masks in one mesh"));}
                character=Some(chr.to_owned());groups.push(Some(group));
                let value=assignment.and_then(|a|a.0.get(&axis.key).copied()).or(axis.default)
                    .or(self.show_unassigned.then_some(1)).ok_or_else(||format!("{} has no selected NPC default",axis.key))?;
                if value>1 {return Err(format!("{}: mask default {value} is not boolean",axis.key));}
                if value==1 {default|=1<<group;}
            }
            if let Some(character)=character {plans.push((entity,PrimitiveDisplayMasks {character,groups,default}));}
        }
        // Validate the complete plan before removing any static condition.
        for (entity,masks) in &plans {masks.verify(*entity,world)?;}
        for (entity,masks) in plans {
            log.count("display mask playback candidates",masks.groups.iter().flatten().count() as u64);
            log.count("static hidden playback candidates",masks.groups.iter().flatten().filter(|&&g|masks.default & (1<<g)==0).count() as u64);
            world.remove::<PrimitiveCondition>(entity);world.insert(entity,masks);
        }
        Ok(())
    }
}

pub const PRESERVE_FROMSOFT_DISPLAY_MASKS:PassSpec=PassSpec {
    name:"preserve-fromsoft-display-masks",
    options:&[OptionSpec::required("defaults","assigned","assigned: require each mask's selected NPC default; show: show unassigned groups (the program explicitly asked to show every group)")],
    make:|g|Ok(Box::new(PreserveDisplayMasks {show_unassigned:match g.required("defaults") {
        "assigned"=>false,"show"=>true,other=>return Err(format!("defaults={other}, expected assigned or show")),
    }})),
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use primase_core::{Entity, Mesh, Primitive};
    use primase_core::state::{Clause, Condition, Domain, Literal, Scope};
    fn fixture()->(World,Entity,Entity) {
        let mut world=World::new();let axis=world.spawn();let mesh=world.spawn();
        world.insert(axis,StateAxis {key:"fromsoft.mask.c1010.3".into(),domain:Domain::Bool,default:None,
            scope:Scope::Global,class:super::super::outfits::MASK_CLASS.into(),source:"fixture".into()});
        let primitive=Primitive {topology:primase_core::Topology::Triangles,streams:BTreeMap::from([
            (primase_core::Semantic::Position,primase_core::Stream::F32x3(vec![[0.,0.,0.],[1.,0.,0.],[0.,1.,0.]]))]),indices:None,material:None};
        world.insert(mesh,Mesh::new(vec![primitive.clone(),primitive]));
        world.insert(mesh,PrimitiveCondition(vec![None,Some(Condition(vec![Clause(vec![Literal {axis,values:0b10}])]))]));
        world.insert_resource(StateAssignment(BTreeMap::from([("fromsoft.mask.c1010.3".into(),0)])));
        (world,mesh,axis)
    }
    #[test]
    fn a_statically_hidden_primitive_stays_available_and_remaps_with_the_mesh() {
        let (mut world,mesh,_)=fixture();
        PreserveDisplayMasks {show_unassigned:false}.run(&mut world,&mut PassLog::default()).expect("preserve");
        assert!(!world.has::<PrimitiveCondition>(mesh));
        let masks=world.get::<PrimitiveDisplayMasks>(mesh).expect("mask metadata");
        assert_eq!(masks.default,0);assert_eq!(masks.groups,vec![None,Some(3)]);
        assert_eq!(world.get::<Mesh>(mesh).expect("mesh").primitives().len(),2);
        assert_eq!(masks.remap_primitives(&[1,0]).expect("hook").expect("remap").groups,vec![Some(3),None]);
    }
    #[test]
    fn resolve_state_keeps_playback_candidates_but_the_static_pipeline_still_discards_them() {
        use primase_core::pass::{PassManager,PassRegistry,RunReport};
        let mut registry=PassRegistry::new();primase_core::register(&mut registry);crate::sekiro::passes::register(&mut registry);
        let manager=PassManager::new(registry).with_pass_checks(true);
        let (mut animated,mesh,_)=fixture();let mut static_world=animated.clone();
        let pipeline=manager.registry.parse("preserve-fromsoft-display-masks{defaults=assigned},resolve-state").expect("pipeline");
        manager.run(&mut animated,&pipeline,&mut RunReport::default()).expect("animated resolve");
        assert_eq!(animated.get::<Mesh>(mesh).expect("retained mesh").primitives().len(),2);
        assert_eq!(animated.get::<PrimitiveDisplayMasks>(mesh).expect("playback metadata").groups,vec![None,Some(3)]);
        let pipeline=manager.registry.parse("resolve-state").expect("static pipeline");
        manager.run(&mut static_world,&pipeline,&mut RunReport::default()).expect("static resolve");
        assert_eq!(static_world.get::<Mesh>(mesh).expect("static mesh").primitives().len(),1);
    }
    #[test]
    fn no_selected_default_refuses_before_changing_the_world() {
        let (mut world,mesh,_)=fixture();world.remove_resource::<StateAssignment>();
        assert!(PreserveDisplayMasks {show_unassigned:false}.run(&mut world,&mut PassLog::default()).is_err());
        assert!(world.has::<PrimitiveCondition>(mesh));assert!(!world.has::<PrimitiveDisplayMasks>(mesh));
        PreserveDisplayMasks {show_unassigned:true}.run(&mut world,&mut PassLog::default()).expect("explicit show");
        assert_eq!(world.get::<PrimitiveDisplayMasks>(mesh).expect("metadata").default,1<<3);
    }
}
