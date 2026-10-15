//! Scene extras compared to an independent export. Retained curve points are
//! exact; geometric closures within 1 cm are counted against quantized joining.
//! Derived bases are within 1e-6, yaw within 0.0051 degrees of rounded reference.
use primase_core::{World, gate::real_data};
use primase_fromsoft::{Archive, dcx::Decoder, msb::MapLayout, sekiro};
use primase_skate_scene::{SkateGrindCurve, SkateLight, SkateMarker, SkateScene, TableRef, tables};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
fn native(p: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    unsafe { primase_compress::oodle::Oodle::load(p, primase_compress::oodle::Version::V6) }
}
#[test]
fn castle_extras_scene() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_EXTRAS_SCENE") else {
        return Ok(());
    };
    let dll = native(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let area = "m11_01_00_00";
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), &decoder)?)?;
    let mut world = World::default();
    sekiro::collision::assemble(&archive, &decoder, area, &layout, None, &mut world)?;
    let report = sekiro::extras::assemble(&archive, &decoder, area, &layout, None, &mut world)?;
    println!("extras {report:?}");
    let lights = tables::read_lights(&std::fs::read(reference.join("tables/lights.bin"))?)?;
    let ours = world.iter::<SkateLight>().map(|(_, l)| l).collect::<Vec<_>>();
    assert_eq!(ours.len(), lights.len());
    let (mut colors, mut shadows) = (0, 0);
    let mut color_max = 0f64;
    for (a, b) in ours.iter().zip(&lights) {
        assert_eq!(a.position, b.position);
        assert_eq!(u32::from(a.spot), b.kind);
        assert_eq!(a.energy, b.energy);
        assert_eq!(a.range, b.range);
        assert_eq!(a.radius, b.radius);
        for (x, y) in a
            .direction
            .iter()
            .chain(&a.tangent)
            .zip(b.direction.iter().chain(&b.tangent))
        {
            assert!((x - y).abs() <= 1e-6);
        }
        assert!((a.spot_size - b.spot_size).abs() <= 1e-6);
        assert_eq!(a.spot_blend, b.spot_blend);
        assert_eq!(a.time_of_day, b.time_of_day);
        for (&x, &y) in a.color.iter().zip(&b.color) {
            let d = (f64::from(x) - f64::from(y)).abs();
            color_max = color_max.max(d);
            colors += usize::from(x != y);
            assert!(d <= 0.009, "named exact sRGB vs power2.2 {d}");
        }
        shadows += usize::from(a.cast_shadow != (b.flags & 1 != 0));
    }
    println!(
        "lights {}: exact sRGB vs power2.2 {colors} changed channels, maximum {color_max}; stored shadow flag vs reference {shadows} differences",
        lights.len()
    );
    let curves = tables::read_curves(&std::fs::read(reference.join("tables/grind.bin"))?)?;
    let ours = world
        .iter::<SkateGrindCurve>()
        .map(|(_, c)| c)
        .collect::<Vec<_>>();
    assert_eq!(ours.len(), curves.curves.len());
    let mut points = 0;
    let mut surface_differences = 0;
    let mut geometric_closures = 0;
    for (a, b) in ours.iter().zip(&curves.curves) {
        let want = &curves.points[b.first as usize..(b.first + b.count) as usize];
        if a.closed && b.flags & 1 == 0 && a.points.len() + 1 == want.len() {
            // Our distance policy closes endpoints within 1 cm; the reference
            // uses quantized-cell equality, which can split nearby endpoints.
            assert_eq!(a.points, want[..want.len() - 1]);
            let gap2 = want[0]
                .iter()
                .zip(want.last().unwrap())
                .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
                .sum::<f64>();
            assert!(gap2 <= 0.01f64.powi(2));
            geometric_closures += 1;
        } else {
            assert_eq!(a.points, want);
            assert_eq!(a.closed, b.flags & 1 != 0);
        }
        assert_eq!(u32::from(a.kind), (b.flags >> 4) & 15);
        assert_eq!(a.radius, b.radius);
        surface_differences += usize::from(a.surface != b.surface);
        points += want.len();
    }
    println!(
        "curves {} reference points {points}, geometric closure differences {geometric_closures}, surface differences {surface_differences}",
        ours.len()
    );
    assert_eq!(surface_differences, 0);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let markers = manifest["markers"].as_array().ok_or("markers")?;
    let ours = world.iter::<SkateMarker>().map(|(_, m)| m).collect::<Vec<_>>();
    assert_eq!(ours.len(), markers.len());
    let mut yaw_changed = 0;
    for (a, b) in ours.iter().zip(markers) {
        let position = serde_json::from_value::<[f32; 3]>(b["position"].clone())?;
        assert_eq!(a.xyz(), Some((position[0], Some(position[1]), position[2])));
        let delta = (a.yaw.unwrap() - b["yaw"].as_f64().ok_or("yaw")?).abs();
        assert!(delta <= 0.0051);
        yaw_changed += usize::from(delta != 0.);
    }
    println!(
        "markers {} positions exact; {yaw_changed} yaw values differ from two-decimal rounding",
        markers.len()
    );
    world.insert_resource(SkateScene {
        name: "sekiro_extras".into(),
        lights: Some(TableRef::single("tables/lights.bin")),
        grind_curves: Some(TableRef::single("tables/grind.bin")),
        ..Default::default()
    });
    if let Some(output) = std::env::var_os("PRIMASE_FROMSOFT_EXTRAS_OUTPUT") {
        primase_skate_scene::write(&world, std::path::Path::new(&output))?;
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(std::path::Path::new(&output).join("scene.json"))?)?;
        for (a, b) in written["markers"]
            .as_array()
            .ok_or("written markers")?
            .iter()
            .zip(markers)
        {
            let mut a = a.as_object().ok_or("marker object")?.clone();
            let mut b = b.as_object().ok_or("reference marker object")?.clone();
            a.remove("yaw");
            b.remove("yaw");
            assert_eq!(a, b, "all marker fields except independently gated yaw");
        }
    }
    Ok(())
}
