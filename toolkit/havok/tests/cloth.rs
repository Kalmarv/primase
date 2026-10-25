//! Synthetic reflected layouts, including two-element reference arrays and record-valued ids.
//! The layouts follow the field names in havok_census and HKLib's 2018 class metadata;
//! no game payload is embedded here.
use primase_havok::{Tagfile, cloth};

struct Definition {
    name: &'static str,
    format: u32,
    size: u32,
    subtype: Option<u32>,
    fields: Vec<(&'static str, u32, u32)>,
}

/// A size or id that does not fit its field is the fixture's mistake: it comes back as the conversion error.
type Built<T> = Result<T, std::num::TryFromIntError>;

fn section(name: &[u8; 4], leaf: bool, bytes: Vec<u8>) -> Built<Vec<u8>> {
    let mut out = (u32::try_from(bytes.len() + 8)? | if leaf { 0x4000_0000 } else { 0 })
        .to_be_bytes().to_vec();
    out.extend(name);
    out.extend(bytes);
    Ok(out)
}

fn packed(value: u32) -> Built<Vec<u8>> {
    Ok(if value < 128 { vec![u8::try_from(value)?] }
    else if value < 16384 { (u16::try_from(value)? | 0x8000).to_be_bytes().to_vec() }
    else { vec![0xc0 | u8::try_from(value >> 16)?, (value >> 8) as u8, value as u8] })
}

fn fixture(sdk: &[u8; 8]) -> Built<Vec<u8>> {
    let primitive = |name, format, size, subtype| Definition { name, format, size, subtype, fields: Vec::new() };
    let record = |name, size, fields| Definition { name, format: 7, size, subtype: None, fields };
    let definitions = vec![
        primitive("hkUint32", 0x8004, 4, None),                         // 1
        primitive("hkReal", 0x174605, 4, None),                       // 2
        primitive("hkStringPtr", 3, 8, None),                        // 3
        primitive("T*", 6, 8, Some(1)),                              // 4
        primitive("hkArray", 8, 16, Some(1)),                        // 5
        primitive("hkArray", 8, 16, Some(4)),                        // 6
        record("hclSimClothData::ParticleData", 16, vec![("mass",0,2),("invMass",4,2),("radius",8,2),("friction",12,2)]), // 7
        primitive("hkArray", 8, 16, Some(7)),                        // 8
        record("hclSimClothPose",24,vec![("name",0,3),("positions",8,5)]), // 9
        record("hclSimClothData",80,vec![("name",0,3),("particleDatas",8,8),("fixedParticles",24,5),
            ("simClothPoses",40,6),("staticConstraintSets",56,6),("maxParticleRadius",72,2),("totalMass",76,2)]), // 10
        record("hclClothData",24,vec![("name",0,3),("simClothDatas",8,6)]), // 11
        record("hkaSkeleton",88,vec![("name",0,3),("parentIndices",8,5),("bones",24,5),
            ("referencePose",40,5),("referenceFloats",56,5),("floatSlots",72,5)]), // 12; all arrays empty
        record("hknpRagdollData",40,vec![("skeleton",0,4),("boneToBodyMap",8,5),("bodyTags",24,5)]), // 13
        record("hknpBodyId",4,vec![("serialAndIndex",0,1)]),            // 14
        record("hknpConstraintId",4,vec![("value",0,1)]),              // 15
        record("hknpConstraintGroupId",4,vec![("value",0,1)]),         // 16
        record("hkpRagdollConstraintData",4,Vec::new()),              // 17; atoms deliberately not interpreted
        record("hknpConstraintCinfo",40,vec![("constraintData",0,4),("bodyA",8,14),("bodyB",12,1),
            ("flags",16,1),("name",24,3),("desiredConstraintId",32,15),("constraintGroupId",36,16)]), // 18
    ];
    let mut names = Vec::new();
    let mut field_names = Vec::new();
    let mut type_names = packed(u32::try_from(definitions.len() + 1)?)?;
    let mut bodies = Vec::new();
    let mut field_index = 0;
    for (index, definition) in definitions.iter().enumerate() {
        names.extend(definition.name.bytes()); names.push(0);
        type_names.extend(packed(u32::try_from(index)?)?); type_names.push(0);
        bodies.extend(packed(u32::try_from(index + 1)?)?); bodies.push(0); // type, no parent
        let is_record = definition.format == 7;
        bodies.extend(packed(9 | if definition.subtype.is_some() { 2 } else { 0 } | if is_record { 32 } else { 0 })?);
        bodies.extend(packed(definition.format)?);
        if let Some(subtype) = definition.subtype { bodies.extend(packed(subtype)?); }
        bodies.extend(packed(definition.size)?); bodies.push(4);
        if is_record {
            bodies.extend(packed(u32::try_from(definition.fields.len())?)?);
            for &(name, offset, kind) in &definition.fields {
                field_names.extend(name.bytes()); field_names.push(0);
                bodies.extend(packed(field_index)?); field_index += 1;
                bodies.extend([32]); bodies.extend(packed(offset)?); bodies.extend(packed(kind)?);
            }
        }
    }
    let mut types = section(b"TSTR",true,names)?;
    types.extend(section(b"TNA1",true,type_names)?);
    types.extend(section(b"FSTR",true,field_names)?); types.extend(section(b"TBDY",true,bodies)?);
    let mut data = vec![0; 364];
    for (offset,index) in [(8,2u64),(24,3),(32,3),(48,6),(64,7),(80,4),(120,5),(128,5),(272,8),(296,10),(320,12)] {
        data[offset..offset+8].copy_from_slice(&index.to_le_bytes());
    }
    for (offset,value) in [(112,1.5f32),(116,2.0),(160,2.0),(164,0.5),(168,0.1),(172,0.25)] {
        data[offset..offset+4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset,value) in [(312,17u32),(316,23),(328,0x0102_0304),(332,12),(336,5),(352,99),(356,7)] {
        data[offset..offset+4].copy_from_slice(&value.to_le_bytes());
    }
    let mut items = vec![0;12];
    for (kind,offset,count,array) in [(11u32,0u32,1u32,false),(4,24,2,true),(10,40,1,false),
        (4,120,2,true),(9,136,1,false),(7,160,1,true),(1,176,1,true),(12,184,1,false),
        (13,272,1,false),(1,312,2,true),(18,320,1,true),(17,360,1,false)] {
        items.extend((kind | if array {0x2000_0000} else {0x1000_0000}).to_le_bytes());
        items.extend(offset.to_le_bytes()); items.extend(count.to_le_bytes());
    }
    let mut root = section(b"SDKV",true,sdk.to_vec())?; root.extend(section(b"DATA",true,data)?);
    root.extend(section(b"TYPE",false,types)?); root.extend(section(b"INDX",false,section(b"ITEM",true,items)?)?);
    section(b"TAG0",false,root)
}

#[test]
fn cloth_data_reads_both_eight_byte_sim_references() -> Built<()> {
    for sdk in primase_havok::SDK_VERSIONS {
        let bytes = fixture(&sdk)?; let tag = Tagfile::read(&bytes,None).unwrap();
        let physics = cloth::read(&tag).unwrap();
        assert_eq!(physics.cloths.len(),1);
        assert_eq!(physics.cloths[0].sim_cloths.len(),2);
    }
    Ok(())
}

#[test]
fn sim_cloth_reads_particles_fixed_particles_and_both_pose_references() -> Built<()> {
    let bytes = fixture(b"20180100")?; let tag = Tagfile::read(&bytes,None).unwrap();
    let physics = cloth::read(&tag).unwrap(); let sim = &physics.cloths[0].sim_cloths[0];
    assert_eq!(sim.particles,[cloth::Particle {mass:2.0,inv_mass:0.5,radius:0.1,friction:0.25}]);
    assert_eq!(sim.fixed_particles,[0]); assert_eq!(sim.poses.len(),2);
    assert_eq!((sim.max_particle_radius,sim.total_mass),(1.5,2.0));
    Ok(())
}

#[test]
fn ragdoll_data_retains_its_skeleton_and_body_tags() -> Built<()> {
    let bytes = fixture(b"20180100")?; let tag = Tagfile::read(&bytes,None).unwrap();
    let physics = cloth::read(&tag).unwrap(); let ragdoll = &physics.ragdolls[0];
    assert!(ragdoll.skeleton.bones.is_empty()); assert!(ragdoll.bone_to_body.is_empty());
    assert_eq!(ragdoll.body_tags,[17,23]);
    Ok(())
}

#[test]
fn constraint_cinfo_reads_record_handles_and_primitive_ids() -> Built<()> {
    let bytes = fixture(b"20180100")?; let tag = Tagfile::read(&bytes,None).unwrap();
    let physics = cloth::read(&tag).unwrap(); let constraint = &physics.constraints[0];
    assert_eq!((constraint.body_a,constraint.body_b),(0x0102_0304,12));
    assert_eq!((constraint.desired_constraint_id,constraint.constraint_group_id),(99,7));
    assert_eq!(constraint.flags,5); assert_eq!(constraint.data_class,"hkpRagdollConstraintData");
    Ok(())
}
