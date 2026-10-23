//! How a draw's vertices make the primitives a geometry program reads, and how a strip's vertices make the primitives
//! it emits. The expected lists are what the D3D11 input assembler of WARP gave a geometry shader that streams each
//! input vertex's number out (`geometry_probe` of the WARP oracle).
use primase_shader::GsOutput;
use primase_shader::exec::geometry::{Strip, Topology};

fn lists(topology: Topology, vertices: usize) -> Vec<Vec<usize>> {
    topology.primitives(vertices)
}

#[test]
fn strips_alternate_the_winding_of_their_triangles() {
    assert_eq!(
        lists(Topology::TriangleStrip, 6),
        [[0, 1, 2], [1, 3, 2], [2, 3, 4], [3, 5, 4]]
    );
    assert_eq!(lists(Topology::LineStrip, 4), [[0, 1], [1, 2], [2, 3]]);
    assert!(lists(Topology::TriangleStrip, 2).is_empty());
    assert!(lists(Topology::LineStrip, 1).is_empty());
}

#[test]
fn lists_ignore_the_vertices_that_complete_no_primitive() {
    assert_eq!(lists(Topology::PointList, 3), [[0], [1], [2]]);
    assert_eq!(lists(Topology::LineList, 5), [[0, 1], [2, 3]]);
    assert_eq!(lists(Topology::TriangleList, 7), [[0, 1, 2], [3, 4, 5]]);
    assert_eq!(
        lists(Topology::LineListAdjacency, 8),
        [[0, 1, 2, 3], [4, 5, 6, 7]]
    );
    assert_eq!(
        lists(Topology::TriangleListAdjacency, 12),
        [[0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11]]
    );
}

#[test]
fn a_strip_with_adjacency_slides_by_one_line_and_by_two_triangle_vertices() {
    assert_eq!(
        lists(Topology::LineStripAdjacency, 6),
        [[0, 1, 2, 3], [1, 2, 3, 4], [2, 3, 4, 5]]
    );
    // the vertices of each triangle at 0, 2, 4, ... of the strip, the odd primitives' winding swapped, the vertex
    // after the last primitive's edge being the strip's odd vertex before it
    assert_eq!(
        lists(Topology::TriangleStripAdjacency, 12),
        [
            [0, 1, 2, 6, 4, 3],
            [2, 5, 6, 8, 4, 0],
            [4, 2, 6, 10, 8, 7],
            [6, 9, 10, 11, 8, 4],
        ]
    );
    assert_eq!(
        lists(Topology::TriangleStripAdjacency, 10),
        [[0, 1, 2, 6, 4, 3], [2, 5, 6, 8, 4, 0], [4, 2, 6, 9, 8, 7]]
    );
    // an odd count ignores its last vertex
    assert_eq!(lists(Topology::TriangleStripAdjacency, 7), [[0, 1, 2, 5, 4, 3]]);
    assert_eq!(
        lists(Topology::TriangleStripAdjacency, 11),
        lists(Topology::TriangleStripAdjacency, 10)
    );
    assert!(lists(Topology::TriangleStripAdjacency, 5).is_empty());
}

#[test]
fn an_emitted_strip_makes_the_primitives_of_the_programs_output() {
    let strip = Strip {
        primitive: 0,
        instance: 0,
        vertices: (0..5).map(|i| vec![[i, 0, 0, 0]]).collect(),
    };
    assert_eq!(
        strip.primitives(GsOutput::TriangleStrip),
        [[0, 1, 2], [1, 3, 2], [2, 3, 4]]
    );
    assert_eq!(strip.primitives(GsOutput::LineStrip).len(), 4);
    assert_eq!(strip.primitives(GsOutput::PointList).len(), 5);
}
