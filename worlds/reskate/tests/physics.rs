//! The PhysicsResource layout ReSkate Studio writes (`primase_reskate::physics`) over primase's encoding
//! (`frostbite_formats::physics`).
//!
//! - `studio_output_is_reproduced` (`oracles.studio.skate/physics`: `oracle.json`, every collision object's input
//!   mesh, and `<mesh>_<slot>.bin`, ReSkate Studio 20261012's PhysicsResource of each): each object welded and encoded
//!   as one shape, written in Studio's layout, is Studio's bytes; and each of Studio's files reads and writes back
//!   byte-identical.

#![allow(clippy::unwrap_used)]

mod inputs;

use frostbite_formats::physics::{self, KIND_MESH, mesh_shape, weld};
use inputs::{Game, OracleSet, oracle};
use primase_reskate::physics::studio_resource;
use serde_json::Value;

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Studio's encoding of each collision object of the oracle map, reproduced byte for byte, and Studio's files read and
/// written back unchanged.
#[test]
fn studio_output_is_reproduced() -> R {
    let Some(dir) = oracle(OracleSet::Studio, Game::Skate, "physics") else {
        return Ok(());
    };
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join("oracle.json"))?)?;
    let objects = manifest.as_object().unwrap();
    assert!(!objects.is_empty());
    for (slot, (name, o)) in objects.iter().enumerate() {
        let positions: Vec<[f32; 3]> = o["positions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| std::array::from_fn(|a| p[a].as_f64().unwrap() as f32))
            .collect();
        let idx: Vec<u32> = o["indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_u64().unwrap() as u32)
            .collect();
        let triangles: Vec<[u32; 3]> = idx.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        let w = weld(&positions, &triangles)?;
        let (shape, centre) = mesh_shape(&w.positions, &w.triangles)?.unwrap();
        let material = o["packed"].as_u64().unwrap() as u32;
        let ours = physics::write(&studio_resource(vec![(shape, centre, material)]))?;
        let file = dir.join(format!("{}_{slot}.bin", o["mesh"].as_str().unwrap()));
        let studio = std::fs::read(&file)?;
        assert!(
            ours == studio,
            "{name}: our encoding differs from Studio's ({})",
            file.display()
        );
        let read = physics::read(&studio)?;
        assert_eq!(physics::write(&read)?, studio, "{name}: round trip");
        assert_eq!(read.parts[0].kind, KIND_MESH);
    }
    Ok(())
}
