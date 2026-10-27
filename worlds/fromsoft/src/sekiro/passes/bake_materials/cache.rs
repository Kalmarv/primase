//! Material bake identity and payload, using the caller's memo store.
use super::pixels::{self, Baked, BoundTexture, Prepared};
use iridia_exec::{Filtering, Input, Lane, SampledTexture};
use primase_cache::{Key, KeyBuilder, source_hash};
use primase_core::World;
use primase_core::pass::PassLog;
use primase_core::pass::memo::{Key as MemoKey, Memo};
use primase_foundations::{sampling::Address, sampling::Subtexel, sampling::Transfer};
use std::collections::BTreeMap;

/// The files that compute a bake's bytes in this crate: editing one retires every entry made before.
fn source() -> [u8; 32] {
    source_hash(&[
        ("pixels.rs", include_bytes!("pixels.rs")),
        ("cache.rs", include_bytes!("cache.rs")),
        ("colour.rs", include_bytes!("colour.rs")),
    ])
}

/// Bump whenever the same inputs can produce different bytes or counts: bake/alpha/decode policy,
/// interpreter arithmetic, sampling, texture decoding, vertex input derivation, or lifter behaviour.
/// Lifter and program versions also participate automatically below. This is also the payload version.
const BAKE_VERSION: u32 = 1;

fn json<T: serde::Serialize>(k: KeyBuilder, tag: &str, value: &T) -> Result<KeyBuilder, String> {
    Ok(k.bytes(tag, &serde_json::to_vec(value).map_err(|e| e.to_string())?))
}

fn image(mut k: KeyBuilder, t: &SampledTexture) -> KeyBuilder {
    let levels = t.levels();
    k = k
        .u32("width", levels.width)
        .u32("height", levels.height)
        .u64("mips", levels.levels.len() as u64);
    for mip in &levels.levels {
        k = k.bytes("rgba8", mip);
    }
    k = k.str(
        "transfer",
        match t.transfer() {
            Transfer::Stored => "stored",
            Transfer::SrgbColour => "srgb",
        },
    );
    for address in t.sampler().address {
        k = k.str(
            "address",
            match address {
                Address::Wrap => "wrap",
                Address::Clamp => "clamp",
                Address::Mirror => "mirror",
                Address::MirrorOnce => "mirror-once",
            },
        );
    }
    for (tag, value) in [
        ("lod-bias", t.sampler().mip_lod_bias),
        ("footprintless", t.footprintless_level()),
    ] {
        k = k.bool(tag, value.is_some());
        if let Some(value) = value {
            k = k.f32(tag, value);
        }
    }
    k.str(
        "filter",
        match t.filtering() {
            Filtering::BaseBilinear {
                subtexel: Subtexel::Exact,
            } => "base-bilinear-exact",
            Filtering::BaseBilinear {
                subtexel: Subtexel::D3d11,
            } => "base-bilinear-d3d11",
            Filtering::Point => "point",
            Filtering::Bilinear => "bilinear",
            Filtering::Trilinear => "trilinear",
        },
    )
}

impl Prepared<'_> {
    /// Canonical tagged, length-prefixed fields; integers LE, floats by bits, ordered maps in key order.
    /// Hash the prepared objects evaluation actually uses, including decoded texture content, rather than a
    /// second walk through bindings or world entity IDs. Derived vertex lanes include frozen vertex constants.
    pub(super) fn key(&self, build: &str) -> Result<Key, String> {
        let Self {
            pixel,
            vertex,
            shader: _,
            outputs,
            // read from `outputs`, which the key holds
            surface: _,
            policies,
            constants,
            textures,
            inputs,
            targets,
            size,
            requested_size,
            source_size,
            alpha,
            base_alpha,
            sample_time,
        } = self;
        let mut k = KeyBuilder::new("fromsoft.material-bake")
            .str("build", build)
            .bytes("source", &source())
            .u32("bake-version", BAKE_VERSION)
            .f32("sample-time", *sample_time)
            .u32("dxbc-lifter", iridia_dxbc::LIFTER_VERSION)
            .u32("dxil-lifter", iridia_dxil::LIFTER_VERSION)
            .u32("program-version", iridia::PROGRAM_VERSION);
        for program in [pixel, vertex] {
            k = json(k, "program-format", &program.format)?.bytes("bytecode", &program.bytecode);
        }
        k = json(k, "outputs", outputs)?;
        // ScalarEncoding's public JSON writes its scale as a float. Retain its raw input bits too:
        // presentation serialization must not collapse distinct IEEE-754 values in an identity.
        for output in outputs {
            if let Some(encoding) = output.role.scalar_encoding() {
                k = k.u32("encoding-scale-bits", encoding.scale_bits);
            }
        }
        k = json(k, "targets", targets)?;
        k = json(k, "policies", policies)?;
        k = k.u64("constant-buffers", constants.buffers().len() as u64);
        for (slot, rows) in constants.buffers() {
            k = k.u32("cb-slot", *slot).u64("rows", rows.len() as u64);
            for row in rows {
                for word in row {
                    k = k.u32("word", *word);
                }
            }
        }
        k = k.u64("inputs", inputs.len() as u64);
        for ((name, index), input) in inputs {
            k = k.str("input-name", name).u32("input-index", *index);
            let lanes = match *input {
                Input::Lanes(lanes) => lanes,
                Input::Constant(c) => c.map(|v| Lane::Bits(v.to_bits())),
                Input::GridUv => [Lane::U, Lane::V, Lane::Bits(0), Lane::Bits(0)],
            };
            for lane in lanes {
                k = match lane {
                    Lane::Bits(bits) => k.str("lane", "bits").u32("bits", bits),
                    Lane::U => k.str("lane", "u"),
                    Lane::V => k.str("lane", "v"),
                };
            }
        }
        k = k.u64("textures", textures.len() as u64);
        for (slot, texture) in textures {
            k = k.u32("texture-slot", *slot);
            k = match texture {
                BoundTexture::Image(t) => image(k.str("texture-kind", "image"), t),
                BoundTexture::Constant(c) => {
                    k = k.str("texture-kind", "constant");
                    for v in c {
                        k = k.f32("channel", *v);
                    }
                    k
                }
            };
        }
        k = k
            .bool("alpha-texture", alpha.is_some())
            .f32("base-alpha", *base_alpha);
        if let Some((texture, transform)) = alpha {
            k = image(k, texture).bool("alpha-transform", transform.is_some());
            if let Some(t) = transform {
                for v in t.offset.into_iter().chain(t.scale).chain([t.rotation]) {
                    k = k.f32("transform", v);
                }
            }
        }
        Ok(k.u32("size", *size)
            .u32("requested-size", *requested_size)
            .u32("source-width", source_size[0])
            .u32("source-height", source_size[1])
            .finish())
    }
}

/// The exact planes and material factor inserted by the pass, plus its per-material report.
pub(super) struct Product {
    pub size: u32,
    pub albedo: Vec<u8>,
    pub normal: Vec<u8>,
    pub orm: Vec<u8>,
    pub specular: Option<Vec<u8>>,
    pub emission: Option<(Vec<u8>, f32)>,
    counts: BTreeMap<String, u64>,
    approximations: BTreeMap<String, u64>,
}
impl Product {
    fn finish(baked: Baked) -> Result<Self, String> {
        let mut log = PassLog::default();
        let allocation = format!(
            "program bake density: independent source width/height maxima {}x{}, requested {}x{}, allocated {}x{}",
            baked.source_size[0],
            baked.source_size[1],
            baked.requested_size,
            baked.requested_size,
            baked.size,
            baked.size,
        );
        log.count(&allocation, 1);
        if baked.size < baked.requested_size {
            log.approximate(&format!("native texture density capped: {allocation}"), 1);
        }
        if baked.source_size == [0; 2] {
            log.approximate("no bound image dimensions; program bake uses max-size", 1);
        }
        log.approximate(
            "program bake follows native texture dimensions; tiled detail can lose density",
            1,
        );
        for (k, n) in baked.policies {
            log.approximate(&k, n);
        }
        let emission = if let Some(rgb) = &baked.emission {
            let (texture, strength, zeroed) =
                super::super::pbr::emissive_from_linear(baked.size, baked.size, rgb)
                    .map_err(|e| e.to_string())?;
            log.count("nonzero baked emission texels quantized black", zeroed);
            Some((
                texture.levels.into_iter().next().expect("emissive base mip"),
                strength,
            ))
        } else {
            None
        };
        log.count("materials baked from their G-buffer program", 1);
        log.count("program bake texels", u64::from(baked.size).pow(2));
        Ok(Self {
            size: baked.size,
            albedo: baked.albedo,
            normal: baked.normal,
            orm: baked.orm,
            specular: baked.specular,
            emission,
            counts: log.counts,
            approximations: log.approximations,
        })
    }
    /// Replay the bake's counts, and its approximations on the material they were made for: the card shows them as
    /// that material's recorded bake. A cache hit replays exactly what the miss recorded.
    pub(super) fn report(&self, log: &mut PassLog, e: primase_core::Entity) {
        for (k, n) in &self.counts {
            log.count(k, *n);
        }
        for (k, n) in &self.approximations {
            log.approximate_on(e, k, *n);
        }
    }
}

#[derive(Debug)]
pub(super) enum Error {
    Refused(String),
    Failed(String),
}

pub(super) fn bake(
    prepared: Prepared<'_>,
    world: &World,
    memo: Option<&Memo>,
    log: &mut PassLog,
) -> Result<Product, Error> {
    let Some(memo) = memo else {
        return Product::finish(pixels::bake(prepared, world).map_err(Error::Refused)?)
            .map_err(Error::Failed);
    };
    let key = MemoKey(prepared.key(&memo.build).map_err(Error::Failed)?.bytes());
    if let Some(value) = memo.store.get(&key) {
        if let Some(baked) = unframe(&value) {
            log.count("program bake cache hits", 1);
            return Ok(baked);
        }
        log.count("program bake cache invalid payloads", 1);
    }
    log.count("program bake cache misses", 1);
    let baked =
        Product::finish(pixels::bake(prepared, world).map_err(Error::Refused)?).map_err(Error::Failed)?;
    // A failed advisory write cannot change a successful bake. The program reads its store's error statistics.
    memo.store.put(&key, &frame(&baked).map_err(Error::Failed)?);
    Ok(baked)
}

/// Small metadata plus raw RGBA8 output planes. The emission factor uses its exact IEEE-754 bits.
/// The report is made once, on a miss; cache hits replay exactly that contribution.
#[derive(serde::Serialize, serde::Deserialize)]
struct Metadata {
    version: u32,
    size: u32,
    counts: BTreeMap<String, u64>,
    approximations: BTreeMap<String, u64>,
    specular: bool,
    emission_strength: Option<u32>,
}

fn frame(b: &Product) -> Result<Vec<u8>, String> {
    let metadata = Metadata {
        version: BAKE_VERSION,
        size: b.size,
        counts: b.counts.clone(),
        approximations: b.approximations.clone(),
        specular: b.specular.is_some(),
        emission_strength: b.emission.as_ref().map(|(_, s)| s.to_bits()),
    };
    let meta = serde_json::to_vec(&metadata).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    out.extend_from_slice(&(meta.len() as u64).to_le_bytes());
    out.extend_from_slice(&meta);
    for plane in [&b.albedo, &b.normal, &b.orm]
        .into_iter()
        .chain(b.specular.iter())
    {
        out.extend_from_slice(plane);
    }
    if let Some((rgba, _)) = &b.emission {
        out.extend_from_slice(rgba);
    }
    Ok(out)
}

fn unframe(value: &[u8]) -> Option<Product> {
    let mut cursor = primase_foundations::bytes::Cursor::new(value);
    let len = usize::try_from(cursor.u64(primase_foundations::bytes::Endian::Little).ok()?).ok()?;
    let m: Metadata = serde_json::from_slice(cursor.take(len).ok()?).ok()?;
    if m.version != BAKE_VERSION || !(1..=4096).contains(&m.size) {
        return None;
    }
    let texels = (m.size as usize).checked_mul(m.size as usize)?;
    let n = texels.checked_mul(4)?;
    let albedo = cursor.take(n).ok()?.to_vec();
    let normal = cursor.take(n).ok()?.to_vec();
    let orm = cursor.take(n).ok()?.to_vec();
    let specular = if m.specular {
        Some(cursor.take(n).ok()?.to_vec())
    } else {
        None
    };
    let emission = if let Some(bits) = m.emission_strength {
        let strength = f32::from_bits(bits);
        if !strength.is_finite() || strength < 1. {
            return None;
        }
        Some((cursor.take(n).ok()?.to_vec(), strength))
    } else {
        None
    };
    if cursor.remaining() != 0 {
        return None;
    }
    Some(Product {
        size: m.size,
        albedo,
        normal,
        orm,
        specular,
        emission,
        counts: m.counts,
        approximations: m.approximations,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "test I/O: private cache fixture directory"
    )]
    use super::super::pixels;
    use super::*;
    use primase_cache::Cache;
    use primase_core::shader::Attribute;
    use std::sync::Arc;

    fn memo(cache: &Arc<Cache>, build: &str) -> Memo {
        Memo::new(cache.clone(), build).unwrap()
    }
    use iridia_interface::{ResourceKind, SamplerState, ShaderFormat, Stage};
    use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
    use primase_core::shader::{
        ConstantBinding, ConstantRole, InputBinding, InputRole, LaneStream, NormalEncoding, NormalSpace,
        OutputBinding, OutputRole, ScalarEncoding,
    };
    use primase_core::{ColorSpace, Entity, TexFormat, TexRole, Texture};
    use primase_core::{shader::Shader, shader::StageBinding, shader::TextureBinding, shader::TextureSource};

    struct Fixture {
        world: World,
        pixel: StageBinding,
        vertex: StageBinding,
        material: PbrMaterial,
        texture: Entity,
    }
    impl Fixture {
        /// Existing original HLSL fixtures: a sampled five-target surface and a UV-producing vertex program.
        fn new() -> Self {
            let mut world = World::default();
            let texture = world.spawn();
            world.insert(
                texture,
                Texture {
                    width: 2,
                    height: 2,
                    format: TexFormat::Rgba8Unorm,
                    levels: vec![vec![
                        128, 255, 128, 192, 32, 128, 64, 255, 64, 192, 128, 96, 96, 255, 64, 128,
                    ]],
                    role: TexRole::Color,
                    color_space: ColorSpace::Linear,
                    normal: None,
                },
            );
            let mut stages = Vec::new();
            for (bytes, stage) in [
                (
                    include_bytes!("../../../../tests/fixtures/fixed_function.dxbc").as_slice(),
                    Stage::Pixel,
                ),
                (
                    include_bytes!("../../../../tests/fixtures/single_uv_vs.dxbc").as_slice(),
                    Stage::Vertex,
                ),
            ] {
                let program = iridia_dxbc::lift(bytes).unwrap();
                let entity = world.spawn();
                let mut binding = StageBinding {
                    stage,
                    variant: None,
                    shader: entity,
                    inputs: vec![],
                    constants: vec![],
                    textures: vec![],
                    buffers: vec![],
                    constant_buffers: vec![],
                    outputs: vec![],
                };
                for r in &program.interface.resources {
                    match &r.kind {
                        ResourceKind::ConstantBuffer { layout } => binding.constants.push(ConstantBinding {
                            resource: r.id,
                            offset: 0,
                            size: layout.size,
                            name: None,
                            lines: None,
                            role: ConstantRole::Material(vec![1f32.to_bits(); layout.size as usize / 4]),
                        }),
                        ResourceKind::Texture { .. } => binding.textures.push(TextureBinding {
                            resource: r.id,
                            sampler: None,
                            source: TextureSource::Texture {
                                texture,
                                sampler: SamplerState::default(),
                            },
                        }),
                        _ => {}
                    }
                }
                if stage == Stage::Pixel {
                    for v in &program.interface.outputs {
                        let register = v.provenance.d3d.unwrap().register;
                        let role = match register {
                            0 => OutputRole::Normal {
                                encoding: NormalEncoding::XyzUnorm,
                                space: NormalSpace::World,
                            },
                            1 => OutputRole::Roughness,
                            2 => OutputRole::Albedo,
                            3 => OutputRole::Reflectance {
                                encoding: ScalarEncoding::LINEAR,
                            },
                            4 => OutputRole::Emission {
                                encoding: ScalarEncoding::LINEAR,
                            },
                            _ => panic!("unexpected fixture output"),
                        };
                        binding.outputs.push(OutputBinding {
                            label: v.label.clone(),
                            lanes: if register == 1 { 1 } else { 7 },
                            role,
                        });
                    }
                } else {
                    for v in &program.interface.inputs {
                        let role = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
                            ("POSITION", 0) => InputRole::Stream(Attribute::Position),
                            ("SV_INSTANCEID", 0) => InputRole::System,
                            ("TEXCOORD", 0) => InputRole::Lanes(vec![
                                LaneStream {
                                    semantic: Attribute::TexCoord(0),
                                    first: 0,
                                    count: 2,
                                    component: 0,
                                },
                                LaneStream {
                                    semantic: Attribute::TexCoord(1),
                                    first: 2,
                                    count: 2,
                                    component: 0,
                                },
                            ]),
                            _ => InputRole::Unknown {
                                why: "fixture attribute outside the pixel demand".into(),
                            },
                        };
                        binding.inputs.push(InputBinding {
                            label: v.label.clone(),
                            role,
                        });
                    }
                }
                world.insert(
                    entity,
                    Shader {
                        format: ShaderFormat::Dxbc,
                        bytecode: bytes.to_vec(),
                        interface: program.interface,
                    },
                );
                stages.push(binding);
            }
            let vertex = stages.pop().unwrap();
            let pixel = stages.pop().unwrap();
            let material = PbrMaterial {
                base_color_texture: Some(TextureRef::new(texture)),
                base_color: [1., 1., 1., 0.75],
                ..Default::default()
            };
            Self {
                world,
                pixel,
                vertex,
                material,
                texture,
            }
        }
        fn prepare(&self, size: u32) -> Prepared<'_> {
            pixels::prepare(
                &self.world,
                &self.pixel,
                Some(&self.vertex),
                &self.material,
                size,
                0.,
            )
            .unwrap()
        }
    }

    fn directory(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "primase-material-bake-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn cache_is_off_unless_the_caller_opts_in() {
        use super::super::{BakeFromSoftMaterials, PixelCap, Seconds};
        use primase_core::pass::Pass;
        let pass = BakeFromSoftMaterials::new(PixelCap::new(2).unwrap(), Seconds::new(0.).unwrap());
        assert_eq!(pass.options()["cache"], "off");
        assert_eq!(pass.with_cache(true).options()["cache"], "on");
    }

    #[test]
    fn original_material_disk_hit_matches_fresh_bytes_and_report() {
        let f = Fixture::new();
        let dir = directory("roundtrip");
        let mut fresh_log = PassLog::default();
        let fresh = bake(f.prepare(2), &f.world, None, &mut fresh_log).unwrap();
        fresh.report(&mut fresh_log, f.texture);
        let expected = frame(&fresh).unwrap();
        let cache = Arc::new(Cache::open(&dir, 1 << 20).unwrap());
        let mut miss_log = PassLog::default();
        let miss = bake(
            f.prepare(2),
            &f.world,
            Some(&memo(&cache, "build-a")),
            &mut miss_log,
        )
        .unwrap();
        miss.report(&mut miss_log, f.texture);
        assert_eq!(expected, frame(&miss).unwrap());
        assert_eq!(miss_log.counts.remove("program bake cache misses"), Some(1));
        assert_eq!(fresh_log.counts, miss_log.counts);
        assert_eq!(fresh_log.approximations, miss_log.approximations);
        // Reopen to exercise the actual disk payload without retaining a producer's memory.
        drop(cache);
        let cache = Arc::new(Cache::open(&dir, 1 << 20).unwrap());
        let mut hit_log = PassLog::default();
        let hit = bake(
            f.prepare(2),
            &f.world,
            Some(&memo(&cache, "build-a")),
            &mut hit_log,
        )
        .unwrap();
        hit.report(&mut hit_log, f.texture);
        assert_eq!(expected, frame(&hit).unwrap());
        assert_eq!(hit_log.counts.remove("program bake cache hits"), Some(1));
        assert_eq!(fresh_log.counts, hit_log.counts);
        assert_eq!(fresh_log.approximations, hit_log.approximations);
        assert_eq!(cache.stats().written, 0);
        assert_eq!(cache.stats().hits, 1);
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changed_constant_texel_and_size_are_disk_misses() {
        let mut f = Fixture::new();
        let dir = directory("invalidation");
        let cache = Arc::new(Cache::open(&dir, 1 << 20).unwrap());
        let original = f.prepare(2).key("build-a").unwrap();
        let product = Product::finish(pixels::bake(f.prepare(2), &f.world).unwrap()).unwrap();
        cache.put(&original, &frame(&product).unwrap()).unwrap();
        assert!(cache.get(&original).is_some());
        let ConstantRole::Material(words) = &mut f.pixel.constants[0].role else {
            panic!("material words")
        };
        let saved = words[0];
        words[0] ^= 1;
        let changed_constant = f.prepare(2).key("build-a").unwrap();
        assert_ne!(original, changed_constant);
        assert!(cache.get(&changed_constant).is_none());
        let ConstantRole::Material(words) = &mut f.pixel.constants[0].role else {
            unreachable!()
        };
        words[0] = saved;
        assert_eq!(original, f.prepare(2).key("build-a").unwrap());
        f.world.get_mut::<Texture>(f.texture).unwrap().levels[0][0] ^= 1;
        let changed_texel = f.prepare(2).key("build-a").unwrap();
        assert_ne!(original, changed_texel);
        assert!(cache.get(&changed_texel).is_none());
        f.world.get_mut::<Texture>(f.texture).unwrap().levels[0][0] ^= 1;
        assert_eq!(original, f.prepare(2).key("build-a").unwrap());
        let changed_size = f.prepare(1).key("build-a").unwrap();
        assert_ne!(original, changed_size);
        assert!(cache.get(&changed_size).is_none());
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn alpha_sampler_vertex_inputs_and_encoding_participate() {
        let mut f = Fixture::new();
        let original = f.prepare(2).key("build-a").unwrap();
        f.material.base_color[3] = 0.5;
        assert_ne!(original, f.prepare(2).key("build-a").unwrap());
        f.material.base_color[3] = 0.75;
        f.material.base_color_texture.as_mut().unwrap().transform = Some(UvTransform {
            offset: [0.25, 0.],
            ..UvTransform::IDENTITY
        });
        assert_ne!(original, f.prepare(2).key("build-a").unwrap());
        f.material.base_color_texture.as_mut().unwrap().transform = None;
        let mut prepared = f.prepare(2);
        prepared
            .inputs
            .insert(("TEXCOORD".into(), 0), Input::Constant([0.5; 4]));
        assert_ne!(original, prepared.key("build-a").unwrap());
        drop(prepared);
        let TextureSource::Texture { sampler, .. } = &mut f.pixel.textures[0].source else {
            unreachable!()
        };
        sampler.wrap[0] = Some(iridia_interface::AddressMode::Clamp);
        assert_ne!(original, f.prepare(2).key("build-a").unwrap());
        let TextureSource::Texture { sampler, .. } = &mut f.pixel.textures[0].source else {
            unreachable!()
        };
        *sampler = SamplerState::default();
        let output = f
            .pixel
            .outputs
            .iter_mut()
            .find(|o| matches!(o.role, OutputRole::Reflectance { .. }))
            .unwrap();
        output.role = OutputRole::Reflectance {
            encoding: ScalarEncoding::linear_over(0.5),
        };
        assert_ne!(original, f.prepare(2).key("build-a").unwrap());
    }

    #[test]
    fn another_build_never_reads_this_builds_entries() {
        let f = Fixture::new();
        let dir = directory("build");
        let cache = Arc::new(Cache::open(&dir, 1 << 20).unwrap());
        let mut a = PassLog::default();
        bake(f.prepare(2), &f.world, Some(&memo(&cache, "build-a")), &mut a).unwrap();
        assert_eq!(a.counts["program bake cache misses"], 1);
        let mut again = PassLog::default();
        bake(f.prepare(2), &f.world, Some(&memo(&cache, "build-a")), &mut again).unwrap();
        assert_eq!(again.counts["program bake cache hits"], 1);
        let mut b = PassLog::default();
        bake(f.prepare(2), &f.world, Some(&memo(&cache, "build-b")), &mut b).unwrap();
        assert_eq!(b.counts["program bake cache misses"], 1);
        assert!(!b.counts.contains_key("program bake cache hits"));
        assert_ne!(
            f.prepare(2).key("build-a").unwrap(),
            f.prepare(2).key("build-b").unwrap()
        );
        assert!(Memo::new(cache.clone(), "").is_err());
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_payload_is_recomputed() {
        let f = Fixture::new();
        let dir = directory("bad-payload");
        let cache = Arc::new(Cache::open(&dir, 1 << 20).unwrap());
        let key = f.prepare(2).key("build-a").unwrap();
        cache.put(&key, b"not a bake payload").unwrap();
        let mut log = PassLog::default();
        let product = bake(f.prepare(2), &f.world, Some(&memo(&cache, "build-a")), &mut log).unwrap();
        assert_eq!(log.counts["program bake cache invalid payloads"], 1);
        assert_eq!(log.counts["program bake cache misses"], 1);
        let payload = frame(&product).unwrap();
        assert!(unframe(&payload[..payload.len() - 1]).is_none());
        let mut extra = payload.clone();
        extra.push(0);
        assert!(unframe(&extra).is_none());
        assert_eq!(
            payload,
            frame(&unframe(&cache.get(&key).unwrap()).unwrap()).unwrap()
        );
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
