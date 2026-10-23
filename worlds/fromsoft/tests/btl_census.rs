//! Every light bank of a game against an independent reader's reading of the same files (`btl.jsonl` of the game's
//! format oracle: SoulsFormatsNEXT ee1dd61's BTL for Elden Ring, `PRIMASE_ELDEN_FORMAT_ORACLE`): the stored version
//! and every light's name, type, colours, powers, shadow flag, cone, position, rotation and radius, floats as bit
//! patterns. A bank the reference refuses must be refused by ours too.
use primase_fromsoft::{Game, btl::LightBank, dcx::Decoder};
use primase_testdata::real_data;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

#[test]
fn every_light_bank_matches_an_independent_reading() -> Result<()> {
    for (game, var) in [(Game::EldenRing, "PRIMASE_ELDEN_FORMAT_ORACLE")] {
        let (Some(archive), Some(oracle), Some(dll)) = (
            common::archive(game)?,
            real_data(var),
            real_data("PRIMASE_OODLE6"),
        ) else {
            continue;
        };
        let dll = common::oodle(dll)?;
        let decoder = Decoder {
            oodle: Some(&dll),
            max_output: 1 << 30,
        };
        let (mut banks, mut lights, mut agreed_rejections) = (0usize, 0usize, 0usize);
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(oracle.join("btl.jsonl"))?.lines() {
            let record: serde_json::Value = serde_json::from_str(line)?;
            let path = record["path"].as_str().ok_or("path")?;
            let bank = match (
                LightBank::read(&archive.read(path, &decoder)?),
                record.get("error"),
            ) {
                (Err(e), Some(reason)) => {
                    // both refuse it: agreement; the reasons are each reader's own words
                    println!("AGREED REJECTION {path}: ours {e}; reference {reason}");
                    agreed_rejections += 1;
                    continue;
                }
                (Err(e), None) => {
                    failures.push(format!("{path}: we refuse what the reference reads: {e}"));
                    continue;
                }
                (Ok(_), Some(_)) => {
                    failures.push(format!("{path}: we read what the reference refuses"));
                    continue;
                }
                (Ok(bank), None) => bank,
            };
            banks += 1;
            let want = record["lights"].as_array().ok_or("lights")?;
            if Some(u64::from(bank.version)) != record["version"].as_u64() || want.len() != bank.lights.len()
            {
                failures.push(format!("{path}: version or light count"));
                continue;
            }
            for (l, w) in bank.lights.iter().zip(want) {
                lights += 1;
                let rgb = |a: [u8; 3]| a.iter().map(|&x| i64::from(x)).collect::<Vec<_>>();
                let ints = |v: &serde_json::Value| {
                    v.as_array()
                        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect::<Vec<_>>())
                };
                let kind = match l.kind {
                    primase_fromsoft::btl::Kind::Point => 0,
                    primase_fromsoft::btl::Kind::Spot => 1,
                    primase_fromsoft::btl::Kind::Directional => 2,
                };
                let same = Some(l.name.as_str()) == w["name"].as_str()
                    && Some(kind) == w["type"].as_i64()
                    && Some(rgb(l.diffuse)) == ints(&w["diffuse"])
                    && Some(rgb(l.specular)) == ints(&w["specular"])
                    && l.diffuse_power.to_bits() == common::bits(&w["diffuse_power"])?
                    && l.specular_power.to_bits() == common::bits(&w["specular_power"])?
                    && Some(l.cast_shadows) == w["cast_shadows"].as_bool()
                    && l.cone_angle.to_bits() == common::bits(&w["cone_angle"])?
                    && l.position.map(f32::to_bits) == common::bits3(&w["position"])?
                    && l.rotation.map(f32::to_bits) == common::bits3(&w["rotation"])?
                    && l.radius.to_bits() == common::bits(&w["radius"])?;
                if !same {
                    failures.push(format!("{path}: light {}", l.name));
                }
            }
        }
        println!(
            "{}: light banks {banks}, lights {lights}, agreed rejections {agreed_rejections}, mismatches {}",
            game.name(),
            failures.len()
        );
        for f in failures.iter().take(40) {
            println!("MISMATCH {f}");
        }
        assert!(banks > 0 && failures.is_empty());
    }
    Ok(())
}
