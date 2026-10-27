//! Output geometry is checked independently against stored half-spaces after
//! f32 rounding: every vertex lies inside all planes and on at least three
//! planes within 0.01 mm. This gate does not claim reference tessellation parity.
use havok_formats::Tagfile;
use havok_formats::convex::ConvexShape;
use primase_havok::ConvexCore;
#[test]
fn object_half_spaces() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = primase_testdata::oracle(
        primase_testdata::OracleSet::Corpus,
        primase_testdata::Game::Sekiro,
        "object-hkx",
    ) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|p| p.map(|p| p.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let (mut file_count, mut bodies, mut triangles, mut vertex_count) = (0, 0, 0, 0);
    let mut maximum = 0f64;
    let mut other = std::collections::BTreeMap::<String, usize>::new();
    for file in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "hkx"))
    {
        let bytes = std::fs::read(&file)?;
        let tag = Tagfile::read(&bytes, None)?;
        file_count += 1;
        let variants = tag.root()?.field("namedVariants")?.array()?;
        for vi in 0..variants.len() {
            let v = variants.get(vi)?.field("variant")?.dereference()?;
            if v.type_name() != "hknpPhysicsSceneData" {
                *other.entry(v.type_name().into()).or_default() += 1;
                continue;
            }
            let systems = v.field("systemDatas")?.array()?;
            for si in 0..systems.len() {
                let bs = systems.get(si)?.dereference()?.field("bodyCinfos")?.array()?;
                for bi in 0..bs.len() {
                    let shape = bs.get(bi)?.field("shape")?.dereference()?;
                    if !["hknpBoxShape", "hknpConvexPolytopeShape", "hknpCapsuleShape"]
                        .contains(&shape.type_name())
                    {
                        *other.entry(shape.type_name().into()).or_default() += 1;
                        continue;
                    }
                    let core = ConvexCore::from_shape(&ConvexShape::read(shape)?)?;
                    let output = core
                        .expanded_triangles()
                        .map_err(|e| format!("{} body {bi}: {e}", file.display()))?;
                    bodies += 1;
                    triangles += output.len();
                    for p in output.iter().flatten() {
                        let mut residuals = Vec::new();
                        for plane in &core.planes {
                            let len = plane[..3]
                                .iter()
                                .map(|&x| f64::from(x).powi(2))
                                .sum::<f64>()
                                .sqrt();
                            let residual = (plane[..3]
                                .iter()
                                .zip(p)
                                .map(|(&n, &x)| f64::from(n) * f64::from(x))
                                .sum::<f64>()
                                + f64::from(plane[3])
                                - f64::from(core.radius))
                                / len;
                            assert!(residual <= 1e-5, "{}: outside {residual}", file.display());
                            residuals.push(residual.abs());
                        }
                        residuals.sort_by(f64::total_cmp);
                        assert!(residuals[2] <= 1e-5);
                        maximum = maximum.max(residuals[2]);
                        vertex_count += 1;
                    }
                }
            }
        }
    }
    assert!(bodies > 0);
    eprintln!(
        "half-space files {file_count}; convex bodies {bodies}; triangles {triangles}; emitted vertices {vertex_count}; max third-plane residual {maximum} m; other classes {other:?}"
    );
    Ok(())
}
