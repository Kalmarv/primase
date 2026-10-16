//! Stored EDGE/BTL values against an independent parser. Colour transfer,
//! energy, spot direction and ledge joining are separate lowering policies.
use primase_fromsoft::{
    btl::{Kind as LightKind, LightBank},
    edge::{Edges, Kind},
};
use primase_testdata::real_data;
use serde_json::Value;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[test]
fn stored_extras() -> Result<()> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_EXTRAS_ORACLE") else {
        return Ok(());
    };
    let (mut files, mut edges, mut lights) = (0, 0, 0);
    for line in std::fs::read_to_string(root.join("extras.jsonl"))?.lines() {
        let v: Value = serde_json::from_str(line)?;
        let bytes = std::fs::read(root.join(v["file"].as_str().ok_or("file")?))?;
        if v["kind"] == "edge" {
            let parsed = Edges::read(&bytes)?;
            assert_eq!(parsed.id as i64, v["id"].as_i64().ok_or("id")?);
            let want = v["edges"].as_array().ok_or("edges")?;
            assert_eq!(parsed.edges.len(), want.len());
            for (e, w) in parsed.edges.iter().zip(want) {
                assert_eq!(
                    match e.kind {
                        Kind::Grapple => 1,
                        Kind::Hang => 2,
                        Kind::Hug => 3,
                    },
                    w["kind"]
                );
                assert_eq!(
                    e.points,
                    serde_json::from_value::<[[f32; 3]; 3]>(w["points"].clone())?
                );
                assert_eq!(
                    e.parameter,
                    serde_json::from_value::<f32>(w["parameter"].clone())?
                );
                assert_eq!(e.id as i64, w["id"].as_i64().ok_or("id")?);
                assert_eq!(
                    u64::from(e.variation),
                    w["variation"].as_u64().ok_or("variation")?
                );
                assert_eq!(u64::from(e.flags), w["flags"].as_u64().ok_or("flags")?);
                edges += 1;
            }
        } else {
            let bank = LightBank::read(&bytes).map_err(|e| format!("{}: {e}", v["file"]))?;
            let want = v["lights"].as_array().ok_or("lights")?;
            assert_eq!(bank.lights.len(), want.len());
            for (l, w) in bank.lights.iter().zip(want) {
                assert_eq!(
                    l.specular,
                    serde_json::from_value::<[u8; 3]>(w["specular"].clone())?
                );
                assert_eq!(
                    l.specular_power,
                    serde_json::from_value::<f32>(w["specular_power"].clone())?
                );
                assert_eq!(l.cast_shadows, w["cast_shadows"].as_bool().ok_or("shadow flag")?);
                assert_eq!(l.name, w["name"]);
                assert_eq!(
                    match l.kind {
                        LightKind::Point => 0,
                        LightKind::Spot => 1,
                        LightKind::Directional => 2,
                    },
                    w["type"]
                );
                assert_eq!(
                    l.diffuse,
                    serde_json::from_value::<[u8; 3]>(w["diffuse"].clone())?
                );
                for (got, key) in [
                    (l.diffuse_power, "power"),
                    (l.cone_angle, "cone"),
                    (l.radius, "radius"),
                ] {
                    assert_eq!(got, serde_json::from_value::<f32>(w[key].clone())?);
                }
                assert_eq!(
                    l.position,
                    serde_json::from_value::<[f32; 3]>(w["position"].clone())?
                );
                assert_eq!(
                    l.rotation,
                    serde_json::from_value::<[f32; 3]>(w["rotation"].clone())?
                );
                lights += 1;
            }
        }
        files += 1;
    }
    assert!(files > 0);
    println!("stored extras: {files} files, {edges} EDGE records, {lights} BTL lights exact");
    Ok(())
}
#[test]
fn truncated_record_tables_are_errors() {
    assert!(Edges::read(&[]).is_err());
    assert!(LightBank::read(&[]).is_err());
    let mut e = vec![0; 16];
    e[..4].copy_from_slice(&4u32.to_le_bytes());
    e[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert!(Edges::read(&e).is_err());
    let mut b = vec![0; 60];
    b[..4].copy_from_slice(&2u32.to_le_bytes());
    b[4..8].copy_from_slice(&16u32.to_le_bytes());
    b[8..12].copy_from_slice(&1u32.to_le_bytes());
    b[20..24].copy_from_slice(&232u32.to_le_bytes());
    assert!(LightBank::read(&b).is_err());
}
