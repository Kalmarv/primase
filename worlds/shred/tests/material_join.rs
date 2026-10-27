//! Hand-worked source identities, literal mip pixels and closure expectations.
use primase_core::{ColorSpace, TexRole, Texture, World};
use primase_shred::{
    components::{
        BankEvidence, EncodedTexture, MaterialId, MaterialKey, MaterialNamespace, MaterialSampler,
        MaterialTextures, ShredMaterial, SourceTexture,
    },
    read::{
        material,
        material_join::{MaterialLibrary, TexturePreference},
        pak::Pak,
    },
};

fn word(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}
fn header() -> Vec<u8> {
    let mut b = vec![0; 84];
    for (at, v) in [
        (0, 0xafaf0003),
        (4, 84),
        (8, 0x003c0000),
        (12, 0xbeefface),
        (20, 0x01000100),
        (56, 60),
        (60, 0x85020200),
        (64, 0xaae4),
        (68, 0x01000100),
        (72, 0x00010000),
    ] {
        word(&mut b, at, v);
    }
    b
}
fn pak(rows: &[(&str, Option<&[u8]>)]) -> Pak {
    let mut names = Vec::new();
    let mut offsets = Vec::new();
    for (n, _) in rows {
        offsets.push(names.len());
        names.extend(n.as_bytes());
        names.push(0);
    }
    let mut pos = 16 + rows.len() * 32 + names.len();
    let mut b = b"PAK\0\xfe\xeb\0\x04".to_vec();
    b.extend((rows.len() as u32).to_be_bytes());
    b.extend((names.len() as u32).to_be_bytes());
    for ((n, data), off) in rows.iter().zip(offsets) {
        let size = data.map_or(0, |v| v.len());
        for v in [
            primase_foundations::hash::sdbm(n.as_bytes()),
            off as u32,
            u32::from(data.is_none()),
            if data.is_none() { 0 } else { 16 },
            pos as u32,
            size as u32,
            0,
            0,
        ] {
            b.extend(v.to_be_bytes());
        }
        pos += size;
    }
    b.extend(names);
    for (_, data) in rows {
        if let Some(data) = data {
            b.extend(*data);
        }
    }
    Pak::from_bytes(b).expect("hand-worked PAK framing")
}
fn material(bank: &str, id: u32, paths: &[&str]) -> ShredMaterial {
    ShredMaterial {
        key: MaterialKey {
            bank: bank.into(),
            id,
        },
        unknown_header: [0; 4],
        vertex_shader: "v".into(),
        fragment_shader: "p".into(),
        parameters: vec![],
        samplers: paths
            .iter()
            .enumerate()
            .map(|(i, path)| MaterialSampler {
                slot: i as u32,
                hash: 999,
                name: None,
                name_source: None,
                texture: (*path).into(),
                unknown_state: [0; 5],
            })
            .collect(),
    }
}

#[test]
fn unique_ids_and_namespace_inference_keep_all_candidates() {
    let mut library = MaterialLibrary::from_materials(
        vec![
            material("environments", 7, &[]),
            material("environments", 0, &[]),
            material("characters", 0, &[]),
            material("actors", 0, &[]),
        ],
        &[],
    )
    .unwrap();
    let r = library.resolve(7, None);
    assert_eq!(r.evidence, Some(BankEvidence::UniqueId));
    assert_eq!(r.selected.unwrap().bank, "environments");
    for (prefix, bank) in [
        ("levels", "environments"),
        ("actors", "actors"),
        ("characters", "characters"),
    ] {
        let r = library.resolve(0, Some(prefix.as_bytes()));
        assert_eq!(r.candidates.len(), 3);
        assert_eq!(r.selected.unwrap().bank, bank);
        assert_eq!(r.evidence, Some(BankEvidence::InferredQuartetNamespace));
    }
    let r = library.resolve(0, Some(b"unknown"));
    assert_eq!(r.candidates.len(), 3);
    assert!(r.selected.is_none());
    assert!(r.evidence.is_none());
    assert!(library.resolve(99, None).candidates.is_empty());
    let mut world = World::new();
    let env = world.spawn();
    world.insert(env, MaterialId(0));
    world.insert(env, MaterialNamespace(b"levels".to_vec()));
    let actor = world.spawn();
    world.insert(actor, MaterialId(0));
    world.insert(actor, MaterialNamespace(b"actors".to_vec()));
    let unknown = world.spawn();
    world.insert(unknown, MaterialId(0));
    world.insert(unknown, MaterialNamespace(b"unknown".to_vec()));
    let report = library
        .attach(&mut world, TexturePreference::FullChainThenPrefetch)
        .unwrap();
    assert_eq!(
        (
            report.materials,
            report.inferred_zero,
            report.unresolved_materials.len()
        ),
        (2, 2, 1)
    );
    assert_eq!(world.get::<ShredMaterial>(env).unwrap().key.bank, "environments");
    assert_eq!(world.get::<ShredMaterial>(actor).unwrap().key.bank, "actors");
    assert!(world.get::<ShredMaterial>(unknown).is_none());
    assert!(
        MaterialLibrary::from_materials(vec![material("actors", 1, &[]), material("actors", 1, &[])], &[])
            .is_err()
    );
}

#[test]
fn both_copies_stay_reachable_full_chain_preferred_and_pixels_deferred() {
    let h = header();
    let base = [1, 2, 3, 4].repeat(256 * 256);
    let tail = [5, 6, 7, 8].repeat(128 * 128);
    let full = [base, tail.clone()].concat();
    let p = pak(&[
        ("TEXTURES_PREFETCH", None),
        ("art\\paint.thp", Some(&h)),
        ("art\\paint.txp", Some(&tail)),
        ("header_only.thp", Some(&h)),
        ("TEXTURES_STREAMED", None),
        ("art\\paint.thp", Some(&h)),
        ("art\\paint.txp", Some(&full)),
    ]);
    let mut lib = MaterialLibrary::from_materials(
        vec![material(
            "environments",
            7,
            &["art\\paint", "", "missing", "art/paint", "header_only"],
        )],
        &[("fixture", &p)],
    )
    .unwrap();
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, MaterialId(7));
    let r = lib
        .attach(&mut world, TexturePreference::FullChainThenPrefetch)
        .unwrap();
    assert!(
        lib.attach(&mut world, TexturePreference::FullChainThenPrefetch)
            .is_err()
    );
    assert_eq!(
        (r.materials, r.empty_bindings, r.texture_paths, r.copies),
        (1, 1, 2, 3)
    );
    assert!(r.unavailable_textures.contains("missing"));
    assert!(r.unavailable_pixels.contains("header_only"));
    assert_eq!(world.iter::<Texture>().count(), 0);
    let bindings = &world.get::<MaterialTextures>(m).unwrap().0;
    assert!(bindings[1].texture.is_none());
    assert!(bindings[2].texture.is_none());
    assert_eq!(bindings[0].texture, bindings[3].texture);
    let texture = bindings[0].texture.unwrap();
    let source = world.get::<SourceTexture>(texture).unwrap();
    assert_eq!(source.preferred, Some(1));
    assert_eq!(source.copies.len(), 2);
    assert_eq!(source.copies[0].first_mip, 1);
    assert_eq!(source.copies[1].first_mip, 0);
    assert_eq!(source.copies[0].header, h);
    let faces: Vec<_> = source
        .copies
        .iter()
        .flat_map(|c| c.faces.iter().copied())
        .collect();
    let closure = world.closure([m]);
    assert!(faces.iter().all(|e| closure.contains(e)));
    let mut part = world.subset(&closure);
    lib.load(&mut part).unwrap();
    assert_eq!(part.iter::<Texture>().count(), 2);
    let prefetch = part.get::<Texture>(faces[0]).unwrap();
    assert_eq!(
        (prefetch.width, prefetch.height, prefetch.levels.len()),
        (128, 128, 1)
    );
    assert_eq!(&prefetch.levels[0][..4], &[6, 7, 8, 5]);
    let streamed = part.get::<Texture>(faces[1]).unwrap();
    assert_eq!(
        (streamed.width, streamed.height, streamed.levels.len()),
        (256, 256, 2)
    );
    assert_eq!(&streamed.levels[0][..4], &[2, 3, 4, 1]);
    assert_eq!(&streamed.levels[1][..4], &[6, 7, 8, 5]);
    assert_eq!(
        (streamed.color_space, streamed.role),
        (ColorSpace::Linear, TexRole::Other)
    );
    assert!(part.verify_components().is_empty());
}

#[test]
fn unsupported_volume_keeps_bytes_but_malformed_supported_pixels_are_errors() {
    let mut h = header();
    word(&mut h, 20, 0x00020002);
    word(&mut h, 60, 0x85010300);
    word(&mut h, 68, 0x00020002);
    word(&mut h, 72, 0x00040000);
    let pixels = [19; 64];
    let p = pak(&[
        ("TEXTURES_STREAMED", None),
        ("volume.thp", Some(&h)),
        ("volume.txp", Some(&pixels)),
    ]);
    let mut lib =
        MaterialLibrary::from_materials(vec![material("actors", 7, &["volume"])], &[("v", &p)]).unwrap();
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, MaterialId(7));
    lib.attach(&mut world, TexturePreference::FullChainThenPrefetch)
        .unwrap();
    let lines = lib.load(&mut world).unwrap();
    assert!(lines.iter().any(|(k, _)| k == "shred unsupported texture"));
    assert_eq!(world.iter::<Texture>().count(), 0);
    let bytes = &world.iter::<EncodedTexture>().next().unwrap().1.bytes;
    assert_eq!(bytes, &pixels);
    let h = header();
    let p = pak(&[
        ("TEXTURES_STREAMED", None),
        ("paint.thp", Some(&h)),
        ("paint.txp", Some(&[0])),
    ]);
    let mut lib =
        MaterialLibrary::from_materials(vec![material("actors", 7, &["paint"])], &[("bad", &p)]).unwrap();
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, MaterialId(7));
    lib.attach(&mut world, TexturePreference::FullChainThenPrefetch)
        .unwrap();
    assert!(lib.load(&mut world).is_err());
}

#[test]
fn engine_dictionary_names_are_exact_hashes() {
    for name in ["Color_Map", "Normal_Map", "Spec_Map", "Effect_Map"] {
        let h = primase_foundations::hash::sdbm(name.as_bytes());
        assert_eq!(material::sampler_name(h), Some(name));
    }
    assert_eq!(material::sampler_name(999), None);
}
