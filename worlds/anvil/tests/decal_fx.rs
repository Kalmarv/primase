//! MAD_Tex1_Graffiti_Mode_DecalFX's placed Material (Riders Republic build 25394987; docs/material-spec.md §27.10)
//! lowered by `lower-anvil-material`: its team colour to the 2.2 as the base colour, its atlas's alpha at the logo's
//! cell (worked by hand from its shader's cell formula), cut out at its AlphaTestValue over 255.
use primase_anvil::components::{AnvilMaterial, MaterialBinding, MaterialValue};
use primase_anvil::passes::{AlphaChoice, LowerAnvilMaterial, ShaderUv};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform};
use primase_core::{NormalGreen, Texture, World};
use primase_testdata::real_data;

/// The value at `cb3` row `row` of `m`: a scalar's (in `.x`) or a vector's.
fn row(m: &AnvilMaterial, row: u32) -> [f64; 4] {
    m.bindings
        .iter()
        .find_map(|(k, b)| match b {
            MaterialBinding::Scalar { vec4 } | MaterialBinding::Vector { vec4 } if *vec4 == row => {
                m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Float(x) if key == k => Some([f64::from(*x), 0.0, 0.0, 0.0]),
                    MaterialValue::Float4(x) if key == k => Some(x.map(f64::from)),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("bound and set")
}

#[test]
fn decal_fx_is_its_team_colour_cut_out_at_its_cell() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut ids = Vec::new();
    for f in [0, 1] {
        let forge = archive.forge(f).expect("forge");
        let entries: Vec<(usize, u64)> = (0..forge.names.len())
            .filter(|&k| forge.names[k].type_id == MATERIAL)
            .map(|k| (k, forge.index[k].id))
            .collect();
        for (entry, id) in entries {
            let body = archive.resource(At { forge: f, entry }).expect("material").body;
            let m = StoredMaterial::parse(&body).expect("parse");
            let Some(t) = m.references[0].and_then(|t| archive.find(t.id)) else {
                continue;
            };
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Tex1_Graffiti_Mode_DecalFX"
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    assert_eq!(ids.len(), 1, "the notes find one");
    let mut w = World::new();
    let mut placed = Placed::default();
    let e = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot)
        .material(ids[0], &mut placed)
        .expect("read");
    let m = w.get::<AnvilMaterial>(e).expect("material").clone();
    let pass = LowerAnvilMaterial {
        normal_green: NormalGreen::Up,
        alpha: AlphaChoice::Opaque,
        shader_uv: ShaderUv::TexCoord,
        emissive_exposure: 1.0,
        underground_view: 0.5,
        fake_river_s: 0.0,
    };
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("lower");
    println!("{log:?}");
    let pbr = w.get::<PbrMaterial>(e).expect("pbr");
    let team = row(&m, 4);
    let want = [0, 1, 2].map(|c| team[c].clamp(0.0, 1.0).powf(2.2) as f32);
    assert_eq!(pbr.base_color, [want[0], want[1], want[2], 1.0]);
    assert_eq!(
        pbr.alpha_mode,
        AlphaMode::Mask {
            cutoff: f32::from(m.flags.alpha_test_value) / 255.0
        }
    );
    assert_eq!((pbr.metallic, pbr.roughness, pbr.double_sided), (0.0, 1.0, false));
    // the cell: u / dx + state / dx, v / dy + (dy - (floor(state / dx + 0.01) + 1)) / dy (the shader's), the logo on
    let (state, dim) = (row(&m, 1)[0], row(&m, 5));
    assert_eq!(row(&m, 2)[0], 1.0, "its logo on");
    let cell_row = (state / dim[0] + 0.01).floor() + 1.0;
    let base = pbr.base_color_texture.as_ref().expect("the shape");
    assert_eq!(
        base.transform.unwrap_or(UvTransform::IDENTITY),
        UvTransform {
            offset: [(state / dim[0]) as f32, ((dim[1] - cell_row) / dim[1]) as f32],
            scale: [(1.0 / dim[0]) as f32, (1.0 / dim[1]) as f32],
            rotation: 0.0,
        }
    );
    // the shape: white, the atlas's alpha
    let shape = w.get::<Texture>(base.texture).expect("texture");
    assert!(
        shape.levels[0]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|t| t[..3] == [255, 255, 255])
    );
    assert_eq!(
        log.counts["DecalFX Materials lowered (a flat team colour, cut out)"],
        1
    );
}
