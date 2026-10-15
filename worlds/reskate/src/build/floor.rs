//! Scene-interchange floor policy, measured from placed parts rather than
//! prototype boxes. Collision triangle minima are used only without placed parts.
use super::*;

#[derive(Debug)]
pub(super) struct FloorLift {
    pub lift: f32,
    pub drop_below: f32,
    pub placements_removed: i64,
    pub parts_removed: i64,
}
impl Default for FloorLift {
    fn default() -> Self {
        Self {
            lift: 0.,
            drop_below: f32::NEG_INFINITY,
            placements_removed: 0,
            parts_removed: 0,
        }
    }
}

/// Every part's height span after the complete placement transform.
fn spans(parts: &[Vec<f32>], p: &Placement) -> Vec<[f32; 2]> {
    parts
        .iter()
        .map(|part| {
            part.as_chunks::<3>()
                .0
                .iter()
                .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], v| {
                    let v = scene::w(v[0], v[1], v[2]);
                    let z = p.pos.z + p.ex.z * v.x + p.ey.z * v.y + p.ez.z * v.z;
                    [lo.min(z), hi.max(z)]
                })
        })
        .collect()
}

fn select_floor(lows: &mut [f32], pinned: f32) -> Option<f32> {
    if !pinned.is_nan() {
        return Some(pinned);
    }
    if lows.is_empty() {
        return None;
    }
    lows.sort_by(f32::total_cmp);
    Some(lows[lows.len() / 100])
}

fn apply_spans(placements: &mut Vec<Placement>, bounds: &[Option<Vec<[f32; 2]>>], result: &mut FloorLift) {
    for (p, bounds) in placements.iter_mut().zip(bounds) {
        let Some(bounds) = bounds else { continue };
        let off: Vec<_> = bounds.iter().map(|b| b[1] < result.drop_below).collect();
        let removed = off.iter().filter(|&&v| v).count();
        if removed > 0 {
            result.parts_removed += removed as i64;
            p.part_off = Some(off);
        }
    }
    let before = placements.len();
    placements.retain(|p| {
        !p.part_off
            .as_ref()
            .is_some_and(|parts| parts.iter().all(|&off| off))
    });
    result.placements_removed = (before - placements.len()) as i64;
}

pub(super) fn prepare(s: &Scene, placements: &mut Vec<Placement>, a: &BuildOptions) -> Result<FloorLift> {
    let height = a.min_height.unwrap_or(s.min_height);
    if height.is_nan() {
        return Ok(FloorLift::default());
    }
    let pinned = a.floor.unwrap_or(s.floor);
    let stub = a.stub_below.unwrap_or(s.stub_below);
    if !height.is_finite() || (!pinned.is_nan() && !pinned.is_finite()) || !stub.is_finite() || stub < 0. {
        return Err(Error::Scene(
            "floor policy requires finite height/floor and nonnegative finite stub_below".into(),
        ));
    }
    let mut bounds = vec![None; placements.len()];
    let mut groups = BTreeMap::<u32, Vec<usize>>::new();
    for (i, p) in placements.iter().enumerate() {
        groups.entry(p.proto).or_default().push(i);
    }
    for proto in &s.protos {
        let (Some(indices), Some(glb)) = (groups.get(&proto.key), &proto.glb) else {
            continue;
        };
        let parts: Vec<_> = scene::read_glb(glb, proto.mesh)?
            .into_iter()
            .map(|p| p.pos)
            .collect();
        for &i in indices {
            bounds[i] = Some(spans(&parts, &placements[i]));
        }
    }
    let mut lows: Vec<_> = bounds
        .iter()
        .flatten()
        .flatten()
        .map(|b| b[0])
        .filter(|v| v.is_finite())
        .collect();
    if lows.is_empty() {
        for soup in &s.soups {
            for triangle in soup.idx.as_chunks::<3>().0 {
                lows.push(
                    triangle
                        .iter()
                        .map(|&i| soup.pos[i as usize * 3 + 1])
                        .fold(f32::INFINITY, f32::min),
                );
            }
        }
    }
    let Some(floor) = select_floor(&mut lows, pinned) else {
        return Ok(FloorLift::default());
    };
    let mut result = FloorLift {
        lift: (height - floor).max(0.),
        drop_below: floor - stub,
        ..FloorLift::default()
    };
    apply_spans(placements, &bounds, &mut result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_meshops::vector::Vec3;
    #[test]
    fn measures_transformed_vertices_and_partwise_stubs() {
        let placement = Placement {
            pos: Vec3::from([0., 0., 10.]),
            ex: Vec3::from([0., 0., 2.]),
            ey: Vec3::from([0., 0., 3.]),
            ez: Vec3::from([0., 0., 4.]),
            ..Placement::default()
        };
        // Writer frame is (x,-z,y): complete scale/rotation matters.
        let bounds = spans(&[vec![1., 2., 3., -1., -2., -3.]], &placement);
        assert_eq!(bounds, vec![[9., 11.]]);
        let mut ps = vec![Placement::default(), Placement::default(), Placement::default()];
        let mut result = FloorLift {
            drop_below: -10.,
            ..FloorLift::default()
        };
        apply_spans(
            &mut ps,
            &[
                Some(vec![[-30., -20.], [-10., -10.]]),
                Some(vec![[-40., -11.]]),
                None,
            ],
            &mut result,
        );
        assert_eq!(result.parts_removed, 2);
        assert_eq!(result.placements_removed, 1);
        assert_eq!(ps[0].part_off, Some(vec![true, false]));
        assert!(ps[1].part_off.is_none());
    }
    #[test]
    fn floor_pinning_percentile_and_empty_scene() {
        assert_eq!(select_floor(&mut [], f32::NAN), None);
        assert_eq!(select_floor(&mut [], -81.602), Some(-81.602));
        let mut lows: Vec<_> = (0..200).rev().map(|v| v as f32).collect();
        assert_eq!(select_floor(&mut lows, f32::NAN), Some(2.));
        assert_eq!(select_floor(&mut lows, -5.), Some(-5.));
    }
    #[test]
    fn empty_and_collision_only_scenes_obey_pins_and_never_lower() {
        let mut world = World::new();
        world.insert_resource(primase_skate_scene::SkateScene {
            name: "floor".into(),
            ..Default::default()
        });
        let mut scene = Scene::from_world(&world, Path::new(".")).unwrap();
        let mut placements = vec![];
        let mut options = BuildOptions {
            min_height: Some(10.),
            ..Default::default()
        };
        assert_eq!(prepare(&scene, &mut placements, &options).unwrap().lift, 0.);
        options.floor = Some(-20.);
        let pinned = prepare(&scene, &mut placements, &options).unwrap();
        assert_eq!(pinned.lift, 30.);
        assert_eq!(pinned.drop_below, -270.);
        options.floor = Some(20.);
        assert_eq!(prepare(&scene, &mut placements, &options).unwrap().lift, 0.);
        options.floor = None;
        scene.soups.push(scene::SoupDef {
            file: "synthetic".into(),
            mode: CollisionMode::TriangleMesh,
            ground: true,
            region_cut: true,
            pos: vec![0., -30., 0., 1., -20., 0., 0., -10., 1.],
            idx: vec![0, 1, 2],
            surf: vec![0],
            read: 0,
            cut: 0,
            kept: 0,
        });
        assert_eq!(prepare(&scene, &mut placements, &options).unwrap().lift, 40.);
        options.stub_below = Some(-1.);
        assert!(prepare(&scene, &mut placements, &options).is_err());
    }
}
