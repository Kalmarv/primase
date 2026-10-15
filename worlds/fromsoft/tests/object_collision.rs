//! Differential value gate against reference revision bd77fcb. Convex bodies
//! are intersections of offset half-spaces, independently checked by the Havok
//! plane gate. Tessellation may differ: vertices and face centroids must lie on
//! the other surface in both directions within 0.1 mm after body transforms. The independent local plane gate uses 0.01 mm.
//! Compressed positions, motion classifications and materials remain exact.
//! Reference names have a byte-widening defect, reversed only for comparison.
use primase_core::gate::real_data;
use primase_fromsoft::collision;
use std::{
    fs,
    io::{Cursor, Read},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type V = [f64; 3];
type Triangle = [[f32; 3]; 3];
fn distance_squared(p: V, t: &Triangle) -> f64 {
    primase_meshops::point_triangle_distance_squared(p, t.map(|v| v.map(f64::from)))
}
fn compare_surface(a: &[Triangle], b: &[Triangle]) -> (usize, f64) {
    assert_eq!(a.is_empty(), b.is_empty());
    let mut maximum = 0f64;
    let mut probes = 0;
    for t in a {
        let vertices = t.map(|v| v.map(f64::from));
        let centroid = std::array::from_fn(|i| vertices.iter().map(|v| v[i]).sum::<f64>() / 3.);
        for p in vertices.into_iter().chain([centroid]) {
            let d = b
                .iter()
                .map(|t| distance_squared(p, t))
                .fold(f64::INFINITY, f64::min)
                .sqrt();
            maximum = maximum.max(d);
            probes += 1;
        }
    }
    (probes, maximum)
}
fn u32le(c: &mut Cursor<Vec<u8>>) -> Result<u32> {
    let mut b = [0; 4];
    c.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn string(c: &mut Cursor<Vec<u8>>) -> Result<String> {
    let mut len = 0usize;
    for shift in (0..35).step_by(7) {
        let mut b = [0];
        c.read_exact(&mut b)?;
        len |= usize::from(b[0] & 127) << shift;
        if b[0] & 128 == 0 {
            let mut bytes = vec![0; len];
            c.read_exact(&mut bytes)?;
            return Ok(String::from_utf8(bytes)?);
        }
    }
    Err("invalid string length".into())
}
#[test]
fn surface_distance_detects_displacement_and_tessellation() {
    let a = [
        [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]],
        [[0., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
    ];
    let b = [
        [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        [[1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
    ];
    assert_eq!(compare_surface(&a, &b).1, 0.);
    assert_eq!(compare_surface(&b, &a).1, 0.);
    let moved = b.map(|t| {
        t.map(|mut v| {
            v[2] = 0.001;
            v
        })
    });
    assert!(compare_surface(&a, &moved).1 > 0.0009);
    assert_eq!(distance_squared([2., 0., 0.], &a[0]), 1.);
}
#[test]
fn castle_object_bodies() -> Result<()> {
    let Some(dir) = real_data("PRIMASE_FROMSOFT_OBJECT_ORACLE") else {
        return Ok(());
    };
    let mut files = fs::read_dir(&dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    let (
        mut count,
        mut bodies_count,
        mut ours,
        mut reference_total,
        mut compressed_components,
        mut convex_bodies,
        mut probes,
    ) = (0, 0, 0, 0, 0, 0, 0);
    let mut maximum = 0f64;
    let mut widened_names = 0;
    let mut failures = Vec::new();
    for path in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "hkx"))
    {
        let bodies = collision::read_bodies(&fs::read(&path)?, None, -1)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut c = Cursor::new(fs::read(path.with_extension("bodies"))?);
        assert_eq!(bodies.len(), u32le(&mut c)? as usize);
        for body in bodies {
            let name = string(&mut c)?;
            if body.name != name {
                let bytes = name
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                assert_eq!(body.name, String::from_utf8(bytes)?);
                widened_names += 1;
            }
            assert_eq!(body.motion_type, u64::from(u32le(&mut c)?));
            assert_eq!(body.shape, string(&mut c)?);
            let n = u32le(&mut c)? as usize;
            let mut reference = vec![[[0.; 3]; 3]; n];
            for v in reference.iter_mut().flatten().flatten() {
                *v = f32::from_bits(u32le(&mut c)?);
            }
            let materials = (0..n)
                .map(|_| u32le(&mut c).map(|x| x as i32))
                .collect::<Result<Vec<_>>>()?;
            if body.shape.contains("CompressedMesh") {
                assert_eq!(body.soup.triangles, reference, "{} {}", path.display(), body.name);
                assert_eq!(body.soup.materials, materials);
                compressed_components += n * 9;
            } else {
                assert!(materials.iter().all(|&m| m == -1));
                assert!(body.soup.materials.iter().all(|&m| m == -1));
                let (pa, da) = compare_surface(&body.soup.triangles, &reference);
                let (pb, db) = compare_surface(&reference, &body.soup.triangles);
                let d = da.max(db);
                maximum = maximum.max(d);
                probes += pa + pb;
                convex_bodies += 1;
                if d > 1e-4 {
                    failures.push(format!(
                        "{} {}: ours->reference {da}, reference->ours {db}; triangles {}/{}",
                        path.display(),
                        body.name,
                        body.soup.triangles.len(),
                        reference.len()
                    ));
                }
            }
            ours += body.soup.triangles.len();
            reference_total += n;
            bodies_count += 1;
        }
        assert_eq!(c.position(), c.get_ref().len() as u64);
        count += 1;
    }
    assert!(count > 0);
    eprintln!(
        "reference bd77fcb: files {count}, bodies {bodies_count}, triangles ours {ours}, reference {reference_total}; compressed position components exact {compressed_components}; byte-widened names {widened_names}"
    );
    eprintln!(
        "convex bodies {convex_bodies}, symmetric surface probes {probes}, maximum {maximum} m, bound 0.0001 m"
    );
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    assert!(failures.is_empty());
    Ok(())
}
