//! Hand-worked object/BCn fixtures and the complete texture census of the PC level-01 and standalone texture forges.
//! Metadata meanings: AnvilToolkit 1.3.7's AC2/Brotherhood code and schema, checked against this game's duplicated
//! metadata and exact mip sizes. Pixel oracle: third-party bcdec_rs 0.2.0 through primase-texcodec's Bcdec adapter.
//! Code 12's BC3 storage is content evidence (not a value supplied by the AC2 table); its channels are checked in
//! `normals.rs`.
use std::collections::BTreeMap;
use std::fs::File;

use anvil_formats::{data::read_block, forge::Forge, objects::read_objects};
use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use primase_shaunwhite::read::texture::{COMPILED_TEXTURE_MAP, TEXTURE_MAP, TEXTURE_MAP_SPEC, TextureMap};
use primase_testdata::{Game, Query, expect_found, install_file};
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
use sha2::{Digest, Sha256};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A 4x4 BC1 red block and its header, laid out here, not serialized by the reader.
fn fixture(dimension: u32) -> Vec<u8> {
    let mut b = vec![1];
    let words = |b: &mut Vec<u8>, v: &[u32]| {
        for n in v {
            b.extend(n.to_le_bytes());
        }
    };
    words(
        &mut b,
        &[
            42,
            TEXTURE_MAP,
            4,
            4,
            1,
            2,
            dimension,
            1,
            1,
            0,
            1,
            2,
            1,
            1,
            2,
            0,
            7,
        ],
    );
    b.extend([4, 0, 0]);
    words(&mut b, &[9]);
    b.push(0);
    let faces = if dimension == 2 { 6 } else { 1 };
    words(&mut b, &[99, COMPILED_TEXTURE_MAP, 8 * faces]);
    for face in 0..faces {
        // Red 565 endpoint, black endpoint, four identical index bytes (one per row).
        b.extend([0, 0xf8, 0, 0]);
        b.extend([u8::try_from(face).expect("six fixture faces fit u8"); 4]);
    }
    words(&mut b, &[2, 32, 4, 4, 1, 1, 2, dimension, 1, 0, 1, 2, 1, 1, 2]);
    b
}

#[test]
fn hand_worked_texture_keeps_its_fields_and_blocks() -> Result {
    let bytes = fixture(1);
    let map = TextureMap::parse(&bytes)?;
    assert_eq!((map.id, map.compiled.id), (42, 99));
    assert_eq!((map.category, map.user_category, map.flags), (7, 9, [4, 0, 0]));
    assert_eq!(map.info.address, [0, 1, 2]);
    assert_eq!(map.info.filter, [1, 1, 2]);
    let images = map.textures()?;
    assert_eq!(images.len(), 1);
    let t = &images[0];
    assert_eq!(
        (t.format, t.color_space, t.role),
        (TexFormat::Bc1, ColorSpace::Srgb, TexRole::Color)
    );
    assert_eq!(t.levels, [vec![0, 0xf8, 0, 0, 0, 0, 0, 0]]);
    assert_eq!(
        Decoder::default().decode_rgba8(t.format, 4, 4, &t.levels[0])?,
        [255, 0, 0, 255].repeat(16)
    );
    let cube = fixture(2);
    let cube = TextureMap::parse(&cube)?.textures()?;
    assert_eq!(cube.len(), 6);
    for (i, t) in cube.iter().enumerate() {
        assert_eq!(t.levels[0][4..], [u8::try_from(i)?; 4]);
    }
    let mut zero_mips = bytes;
    zero_mips[33] = 0;
    zero_mips[117] = 0;
    let zero_mips = TextureMap::parse(&zero_mips)?;
    assert_eq!(zero_mips.info.mips, 0);
    assert_eq!(zero_mips.textures()?[0].levels.len(), 1);
    Ok(())
}

#[test]
fn malformed_or_unestablished_textures_name_the_failure() {
    let good = fixture(1);
    for len in 0..good.len() {
        assert!(TextureMap::parse(&good[..len]).is_err(), "truncated at {len}");
    }
    for (offset, value, field) in [
        (0, 2, "managed flag"),
        (5, 0, "texture class"),
        (9, 0, "dimensions"),
        (21, 99, "pixel format"),
        (25, 3, "texture dimension"),
        (29, 2, "gamma settings"),
        (33, 4, "mip count"),
        (37, 9, "address modes"),
        (49, 9, "filter modes"),
        (61, 255, "map type"),
        (76, 3, "compiled pointer tag"),
        (81, 0, "compiled class"),
        (97, 3, "platform version"),
        (101, 0, "SDK version"),
        (105, 8, "compiled pixel length"),
    ] {
        let mut bad = good.clone();
        bad[offset] = value;
        let err = TextureMap::parse(&bad).unwrap_err().to_string();
        assert!(err.contains(field), "offset {offset}: {err}, wanted {field}");
    }
    let mut mismatch = good.clone();
    mismatch[129] = 0; // compiled gamma differs, with unchanged storage dimensions
    assert!(
        TextureMap::parse(&mismatch)
            .unwrap_err()
            .to_string()
            .contains("compiled metadata")
    );
    let mut extra = good;
    extra.push(0);
    assert!(
        TextureMap::parse(&extra)
            .unwrap_err()
            .to_string()
            .contains("trailing bytes")
    );
}

#[test]
fn hand_worked_object_directory_is_exact() -> Result {
    let body = fixture(1);
    let mut object = Vec::new();
    object.extend(TEXTURE_MAP.to_le_bytes());
    object.extend(u32::try_from(body.len())?.to_le_bytes());
    object.extend(1u32.to_le_bytes());
    object.extend(b"R\0");
    object.extend(body);
    let mut meta = 1u16.to_le_bytes().to_vec();
    meta.extend(42u32.to_le_bytes());
    meta.extend(u32::try_from(object.len())?.to_le_bytes());
    let objects = read_objects(&meta, &object)?;
    assert_eq!(
        (objects[0].id, objects[0].class, objects[0].name.as_str()),
        (42, TEXTURE_MAP, "R")
    );
    assert_eq!(TextureMap::parse(&object[objects[0].body.clone()])?.id, 42);
    for n in 0..object.len() {
        assert!(read_objects(&meta, &object[..n]).is_err());
    }
    let mut extra = meta.clone();
    extra.push(0);
    assert!(read_objects(&extra, &object).is_err());
    let mut extra_object = object;
    extra_object.push(0);
    assert!(read_objects(&meta, &extra_object).is_err());
    Ok(())
}

#[derive(Default)]
struct Census {
    formats: BTreeMap<u32, usize>,
    unique: BTreeMap<u32, [u8; 32]>,
    candidates: BTreeMap<(u32, u32, u32), (String, u32, Texture)>,
    alpha: Option<(String, u32, Texture)>,
    faces: usize,
    levels: usize,
    cubes: usize,
}

impl Census {
    fn texture(&mut self, name: &str, body: &[u8], id: u32) -> Result {
        let map = TextureMap::parse(body).map_err(|e| format!("{name}, id {id:#x}: {e}"))?;
        assert_eq!(map.id, id, "{name}");
        *self.formats.entry(map.info.format_code).or_default() += 1;
        if map.info.dimension == 2 {
            self.cubes += 1;
        }
        let digest: [u8; 32] = Sha256::digest(body).into();
        if let Some(previous) = self.unique.insert(id, digest) {
            assert_eq!(
                previous, digest,
                "{id:#x} {name}: repeated id has different bytes"
            );
            return Ok(());
        }
        let textures = map.textures()?;
        assert_eq!(
            textures
                .iter()
                .flat_map(|t| &t.levels)
                .flatten()
                .copied()
                .collect::<Vec<_>>(),
            map.compiled.pixels
        );
        self.faces += textures.len();
        for t in &textures {
            self.levels += t.levels.len();
            for (level, bytes) in t.levels.iter().enumerate() {
                let shift = u32::try_from(level)?;
                let (w, h) = ((t.width >> shift).max(1), (t.height >> shift).max(1));
                let ours = Decoder::default().decode_rgba8(t.format, w, h, bytes)?;
                let oracle = Bcdec.decode_rgba8(t.format, w, h, bytes)?;
                assert_eq!(ours, oracle, "{name}, face mip {level}: third-party bcdec");
                if level == 0
                    && self.alpha.is_none()
                    && ours.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
                    && map.map_type == 0
                {
                    self.alpha = Some((name.into(), id, t.clone()));
                }
            }
        }
        self.candidates
            .entry((map.info.format_code, map.info.dimension, map.map_type))
            .or_insert_with(|| (name.into(), id, textures[0].clone()));
        Ok(())
    }

    fn samples(&self) -> Result {
        let folder = primase_testdata::scratch().join("textures-sample");
        std::fs::create_dir_all(&folder)?;
        let mut chosen = BTreeMap::new();
        for code in self.formats.keys() {
            let sample = self
                .candidates
                .iter()
                .find(|(key, _)| key.0 == *code)
                .expect("format has a sample")
                .1;
            chosen.insert(sample.1, sample);
        }
        for predicate in [1u32, 2] {
            let sample = self
                .candidates
                .iter()
                .find(|(key, _)| if predicate == 1 { key.2 == 1 } else { key.1 == 2 })
                .expect("normal and cube samples exist")
                .1;
            chosen.insert(sample.1, sample);
        }
        if let Some(alpha) = &self.alpha {
            chosen.insert(alpha.1, alpha);
        }
        for sample in self.candidates.values() {
            if chosen.len() == 10 {
                break;
            }
            chosen.insert(sample.1, sample);
        }
        assert_eq!(chosen.len(), 10);
        let mut manifest = String::from("id\tname\tformat\twidth\theight\trole\tcolor_space\n");
        for (id, (name, _, texture)) in chosen {
            let filename = format!("{id:08x}.png");
            std::fs::write(
                folder.join(&filename),
                primase_image::texture_level_png(texture, 0)?,
            )?;
            manifest.push_str(&format!(
                "{id:08x}\t{name}\t{:?}\t{}\t{}\t{:?}\t{:?}\n",
                texture.format, texture.width, texture.height, texture.role, texture.color_space
            ));
        }
        std::fs::write(folder.join("textures.tsv"), manifest)?;
        println!(
            "10 raw-channel texture previews: {} (normal packing retained)",
            folder.display()
        );
        Ok(())
    }
}

#[test]
fn every_level_01_and_standalone_texture() -> Result {
    let level = install_file(Game::ShaunWhite, "DataPC_01.forge");
    let extra = install_file(Game::ShaunWhite, "DataPC_extra.forge");
    let (Some(level), Some(extra)) = (level, extra) else {
        return Ok(());
    };
    let mut census = Census::default();
    for (path, query, class) in [
        (
            level,
            Query::Install(Game::ShaunWhite, Some("DataPC_01.forge")),
            0xAC2B_BF68,
        ),
        (
            extra,
            Query::Install(Game::ShaunWhite, Some("DataPC_extra.forge")),
            TEXTURE_MAP_SPEC,
        ),
    ] {
        let forge = Forge::open(&path)?;
        let mut file = File::open(&path)?;
        let (mut entries, mut textures) = (0, 0);
        for (i, name) in forge.names.iter().enumerate().filter(|(_, n)| {
            n.name.ends_with(if class == TEXTURE_MAP_SPEC {
                "MapSpec"
            } else {
                "_DataBlock"
            })
        }) {
            let payload = forge.payload(&mut file, i)?;
            let (metadata, next) = read_block(&payload, 0, 1 << 28)?;
            // Directory-only entries may omit the resource block; absence is reported separately.
            if next == payload.len() {
                continue;
            }
            let (resource, end) = read_block(&payload, next, 1 << 28)?;
            assert_eq!(end, payload.len(), "{}: more than two data blocks", name.name);
            let objects = read_objects(&metadata.data, &resource.data)?;
            assert_eq!(objects[0].class, class, "{}: selected entry class", name.name);
            for object in objects {
                if object.class == TEXTURE_MAP {
                    census.texture(&object.name, &resource.data[object.body], object.id)?;
                    textures += 1;
                }
            }
            entries += 1;
        }
        expect_found(query, "texture-bearing entries", entries);
        println!(
            "{}: {entries} entries, {textures} texture occurrences",
            path.display()
        );
    }
    if census.unique.is_empty() {
        return Ok(());
    }
    println!(
        "Texture occurrences by stored format: {:?}; {} unique ids, {} cube occurrences; {} unique faces, {} mip levels cross-checked against bcdec",
        census.formats,
        census.unique.len(),
        census.cubes,
        census.faces,
        census.levels
    );
    census.samples()?;
    Ok(())
}
