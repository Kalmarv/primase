//! Independent source shadow records, checked against the original MSB bytes.
use primase_fromsoft::msb::MapLayout;
use sha2::{Digest, Sha256};

#[derive(serde::Deserialize)]
struct Oracle {
    sha256: String,
    parts: Vec<Part>,
}
#[derive(serde::Deserialize)]
struct Part {
    name: String,
    cast: bool,
    receive: bool,
    only: bool,
    point_light_cast: bool,
    static_cast: u8,
    cascade3_cast: u8,
}

#[test]
fn source_shadow_flags() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::real_data("PRIMASE_FROMSOFT_SHADOW_ORACLE") else {
        return Ok(());
    };
    let data = std::fs::read(root.join("map.msb"))?;
    let oracle: Oracle = serde_json::from_slice(&std::fs::read(root.join("shadows.json"))?)?;
    assert_eq!(
        Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        oracle.sha256.to_lowercase()
    );
    let map = MapLayout::read(&data)?;
    assert!(!oracle.parts.is_empty(), "empty shadow coverage");
    assert_eq!(map.parts.len(), oracle.parts.len());
    let mut contradictory = 0;
    for (actual, expected) in map.parts.iter().zip(&oracle.parts) {
        assert_eq!(actual.name, expected.name, "part {}", actual.index);
        let s = actual.shadows;
        assert_eq!(
            (
                s.cast,
                s.receive,
                s.only,
                s.point_light_cast,
                s.static_cast,
                s.cascade3_cast
            ),
            (
                expected.cast,
                expected.receive,
                expected.only,
                expected.point_light_cast,
                expected.static_cast,
                expected.cascade3_cast
            ),
            "part {} {}",
            actual.index,
            actual.name
        );
        contradictory += usize::from(s.only && !s.cast);
    }
    println!(
        "{} source parts, six exact shadow fields each, {contradictory} contradictory cast/only combinations",
        map.parts.len()
    );
    Ok(())
}
