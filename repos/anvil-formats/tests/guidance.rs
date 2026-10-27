//! Skate guidance: a SWSkateGuidanceSystem built byte by byte read back with its chains.
use anvil_formats::shaunwhite::guidance::{
    Class, EDGE_FILTER, FREE_FORM_GUIDANCE_SYSTEM, GUIDANCE_OBJECT, GuidanceSystem, MORPH_GUIDANCE_SYSTEM,
    MORPH_TARGET_DATA, NO_LINK, PART_NODE, PARTITIONER, SCALE_GUIDANCE_SYSTEM, SW_GUIDANCE_OBJECT_DATA,
    SW_GUIDANCE_SYSTEM, Shaping,
};
use anvil_formats::shaunwhite::scene::RESOURCE_REF;

fn u32s(out: &mut Vec<u8>, v: &[u32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}
fn f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// An edge: (start, end) points, (start link, end link), length.
type BuiltEdge = ((u32, u32), (u32, u32), f32);

/// A GuidanceObject array: a count, then the objects.
fn objects(edges: &[BuiltEdge]) -> Vec<u8> {
    let mut b = Vec::new();
    u32s(&mut b, &[edges.len() as u32]);
    for ((i0, i1), (start, end), length) in edges {
        b.extend_from_slice(&[0, 0, 0, 0]);
        u32s(&mut b, &[GUIDANCE_OBJECT]);
        b.push(1);
        u32s(&mut b, &[0, *i0, *i1]);
        f32s(&mut b, &[0., 0., 1.]);
        u32s(&mut b, &[0]);
        f32s(&mut b, &[0., -1., 0.]);
        u32s(&mut b, &[0]);
        b.push(0);
        u32s(&mut b, &[0x2000, SW_GUIDANCE_OBJECT_DATA, 0, *end, *start]);
        f32s(&mut b, &[*length]);
    }
    b
}

/// The edge filter and the partitioner every system holds after its points: `leaves` leaf edges, one node.
fn index(leaves: u16) -> Vec<u8> {
    let mut b = vec![0u8];
    u32s(&mut b, &[0x2001, EDGE_FILTER]);
    f32s(
        &mut b,
        &[
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_PI_8,
            std::f32::consts::FRAC_PI_4,
        ],
    );
    b.extend_from_slice(&[0, 0]);
    f32s(&mut b, &[46., 60., 0.2]);
    b.push(0);
    b.push(0);
    u32s(&mut b, &[0x2002, PARTITIONER]);
    b.extend_from_slice(&[0, 0]);
    u32s(&mut b, &[u32::from(leaves)]);
    for i in 0..leaves {
        b.extend_from_slice(&i.to_le_bytes());
    }
    f32s(&mut b, &[-1., -1., -1., 1., 1., 1.]);
    u32s(&mut b, &[1]);
    b.extend_from_slice(&[0, 0, 0, 0]);
    u32s(&mut b, &[PART_NODE, 0]);
    b.extend_from_slice(&[0, 0, 1, 0]);
    f32s(&mut b, &[0.]);
    b
}

/// A component of class `class` of `edges` over `points` (encoded as the game encodes them), up to the end of its
/// partitioner.
fn system_of(class: u32, edges: &[BuiltEdge], points: &[[f32; 3]]) -> Vec<u8> {
    let mut b = vec![0u8, 4];
    u32s(&mut b, &[0x1234, class, 0x1235, RESOURCE_REF]);
    b.push(0xff);
    b.extend(objects(edges));
    u32s(&mut b, &[(points.len() * 3) as u32]);
    for p in points {
        for v in p {
            b.extend_from_slice(&((v * 200.).round() as i32 + 32767).to_le_bytes()[..2]);
        }
    }
    b.extend(index(edges.len() as u16));
    b
}

/// A plain system.
fn system(edges: &[BuiltEdge], points: &[[f32; 3]]) -> Vec<u8> {
    system_of(SW_GUIDANCE_SYSTEM, edges, points)
}

#[test]
fn a_built_system_reads_back_as_chains() {
    let points = [
        [0., 0., 0.4],
        [2., 0., 0.4],
        [4., 0., 0.4],
        [4., 3., 0.4],
        [-1.5, 7.25, 1.],
    ];
    // edges 0-1-2 chained (edge 1 runs backwards, 2 -> 1), edge 2 continues to point 3; edge 3 alone
    let edges = [
        ((0, 1), (NO_LINK, 1), 2.),
        ((2, 1), (2, 0), 2.),
        ((2, 3), (1, NO_LINK), 3.),
        ((4, 0), (NO_LINK, NO_LINK), 9.),
    ];
    let b = system(&edges, &points);
    let s = GuidanceSystem::parse(&b, 6).unwrap();
    assert_eq!(s.points, points.to_vec());
    assert_eq!(s.edges.len(), 4);
    assert_eq!(s.edges[1].links, [2, 0]);
    assert_eq!(s.edges[0].normals, [[0., 0., 1.], [0., -1., 0.]]);
    let chains = s.chains();
    let runs: Vec<(Vec<u32>, bool)> = chains.iter().map(|c| (c.points.clone(), c.closed)).collect();
    assert_eq!(runs, vec![(vec![0, 1, 2, 3], false), (vec![4, 0], false)]);

    // a link to an edge that holds neither point and does not link back is stale: read as no link, and counted
    let stale = system(
        &[((0, 1), (NO_LINK, 1), 2.), ((2, 3), (NO_LINK, NO_LINK), 3.)],
        &points,
    );
    let s = GuidanceSystem::parse(&stale, 6).unwrap();
    assert_eq!((s.stale_links, s.edges[0].links), (1, [NO_LINK, NO_LINK]));
    assert_eq!(s.chains().len(), 2);
    // one the named edge links back, or to an edge past the list, is refused
    let back = system(&[((0, 1), (NO_LINK, 1), 2.), ((2, 3), (0, NO_LINK), 3.)], &points);
    assert!(GuidanceSystem::parse(&back, 6).is_err());
    let beyond = system(&[((0, 1), (NO_LINK, 7), 2.)], &points);
    assert!(GuidanceSystem::parse(&beyond, 6).is_err());
    // so is an index past the points
    let past = system(&[((0, 9), (NO_LINK, NO_LINK), 2.)], &points);
    assert!(GuidanceSystem::parse(&past, 6).is_err());
}

#[test]
fn a_loop_reads_closed() {
    let points = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]];
    let edges = [((0, 1), (2, 1), 1.), ((1, 2), (0, 2), 1.), ((2, 0), (1, 0), 1.4)];
    let s = GuidanceSystem::parse(&system(&edges, &points), 6).unwrap();
    let chains = s.chains();
    assert_eq!(chains.len(), 1);
    assert_eq!(
        (chains[0].points.clone(), chains[0].closed),
        (vec![0, 1, 2], true)
    );
}

#[test]
fn the_shaping_classes_read_after_the_partitioner() {
    let points = [[0., 0., 0.], [0., 5., 0.], [0., 5., 2.]];
    let edges = [((0, 1), (NO_LINK, 1), 5.), ((1, 2), (0, NO_LINK), 2.)];

    // scale: the plain layout, nothing after the partitioner
    let b = system_of(SCALE_GUIDANCE_SYSTEM, &edges, &points);
    let s = GuidanceSystem::parse(&b, 6).unwrap();
    assert_eq!(
        (s.class, s.shaping.clone(), s.end),
        (Class::Scale, Shaping::None, b.len())
    );
    assert_eq!(s.chains().len(), 1);

    // free-form: one u32 after it, the shared object's id
    let mut b = system_of(FREE_FORM_GUIDANCE_SYSTEM, &edges, &points);
    u32s(&mut b, &[0x9016_87FC]);
    let s = GuidanceSystem::parse(&b, 6).unwrap();
    assert_eq!(
        (s.class, s.shaping, s.end),
        (Class::FreeForm, Shaping::Template(0x9016_87FC), b.len())
    );
    // the u32 is not optional
    assert!(GuidanceSystem::parse(&b[..b.len() - 1], 6).is_err());
}

/// A morph system of `edges` over `points` with one further state: the same edges, every point 3 m higher.
fn morph(edges: &[BuiltEdge], points: &[[f32; 3]]) -> Vec<u8> {
    let mut b = system_of(MORPH_GUIDANCE_SYSTEM, edges, points);
    u32s(&mut b, &[2]);
    for (id, lift) in [(0x40u32, 0f32), (0x41, 3.)] {
        u32s(&mut b, &[id, MORPH_TARGET_DATA]);
        b.extend(objects(edges));
        u32s(&mut b, &[points.len() as u32]);
        for p in points {
            f32s(&mut b, &[p[0], p[1] + lift, p[2]]);
        }
    }
    b
}

#[test]
fn a_morph_system_reads_its_states_and_chains_by_points() {
    let points = [[0., 0., 0.], [1., 0., 0.], [2., 0., 0.], [5., 5., 5.]];
    // links that name nothing in the object list (the game's morph systems hold such links)
    let edges = [((0, 1), (7, 9), 1.), ((2, 1), (8, 8), 1.)];
    let b = morph(&edges, &points);
    let s = GuidanceSystem::parse(&b, 6).unwrap();
    assert_eq!((s.class, s.end), (Class::Morph, b.len()));
    let Shaping::States(states) = &s.shaping else {
        panic!("{:?}", s.shaping)
    };
    assert_eq!(states.iter().map(|x| x.id).collect::<Vec<_>>(), vec![0x40, 0x41]);
    assert_eq!(states[1].points[1], [1., 3., 0.]);
    assert_eq!(states[1].edges.len(), 2);
    // the edges 0-1 and 2-1 share point 1: one chain, starting at the lowest end
    let chains = s.chains();
    assert_eq!(chains.len(), 1);
    assert_eq!(
        (chains[0].points.clone(), chains[0].edges.clone()),
        (vec![0, 1, 2], vec![0, 1])
    );

    // a state with another point count than the system is refused
    let mut short = system_of(MORPH_GUIDANCE_SYSTEM, &edges, &points);
    u32s(&mut short, &[1, 0x40, MORPH_TARGET_DATA]);
    short.extend(objects(&edges));
    u32s(&mut short, &[2]);
    f32s(&mut short, &[0.; 6]);
    assert!(GuidanceSystem::parse(&short, 6).is_err());
    // so is a state whose edge names a point past the system's
    let mut past = system_of(MORPH_GUIDANCE_SYSTEM, &edges, &points);
    u32s(&mut past, &[1, 0x40, MORPH_TARGET_DATA]);
    past.extend(objects(&[((0, 9), (NO_LINK, NO_LINK), 1.)]));
    u32s(&mut past, &[points.len() as u32]);
    f32s(&mut past, &[0.; 12]);
    assert!(GuidanceSystem::parse(&past, 6).is_err());
}

#[test]
fn a_morph_loop_reads_closed_and_a_plain_system_needs_its_partitioner() {
    let points = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]];
    let edges = [((0, 1), (0, 0), 1.), ((1, 2), (0, 0), 1.), ((2, 0), (0, 0), 1.4)];
    let s = GuidanceSystem::parse(&morph(&edges, &points), 6).unwrap();
    let chains = s.chains();
    assert_eq!(chains.len(), 1);
    assert_eq!(
        (chains[0].points.clone(), chains[0].closed),
        (vec![0, 1, 2], true)
    );
    // a system cut before its partitioner is refused
    let b = system(&edges, &points);
    let cut = b.len() - index(3).len();
    assert!(GuidanceSystem::parse(&b[..cut], 6).is_err());
}
