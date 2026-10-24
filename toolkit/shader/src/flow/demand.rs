//! The vertex inputs required by a particular vertex/pixel pair, at logical-lane precision.

use super::{Node, NodeId, Values, labelled};
use crate::{Program, Stage};
use std::collections::BTreeMap;

/// Input lane masks, indexed by each program's interface, that can affect the draw. A vertex input used only to
/// compute a varying the pixel program never reads needs no source. Position, clip/cull and other system outputs,
/// as well as either stage's discards and side effects, always remain roots. This is conservative dataflow: conditions
/// and texture coordinates count as dependencies, and runtime constants are not specialised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexDemand {
    pub vertex: BTreeMap<usize, u8>,
    pub pixel: BTreeMap<usize, u8>,
}

fn inputs(values: &Values, roots: impl IntoIterator<Item = NodeId>) -> BTreeMap<usize, u8> {
    let mut masks = BTreeMap::new();
    for id in values.reached_from(roots) {
        if let Node::Input { input, lane } = values.node(id) {
            *masks.entry(*input).or_default() |= 1 << lane;
        }
    }
    masks
}

impl VertexDemand {
    /// Follow the bound pixel program's actual reads back through vertex outputs with the same semantic label and
    /// logical lane. An unlinked read or a graph the query cannot follow is an error, never evidence an input is unused.
    pub fn of_programs(vertex: &Program, pixel: &Program) -> Result<Self, String> {
        if vertex.interface.stage != Stage::Vertex || pixel.interface.stage != Stage::Pixel {
            return Err("vertex demand requires a vertex and a pixel program".into());
        }
        let vg = Values::of_program(vertex)?;
        let pg = Values::of_program(pixel)?;
        let pixel_inputs = inputs(&pg, pg.roots());
        let mut roots: Vec<_> = vg
            .outputs()
            .iter()
            .filter(|((output, _), _)| vertex.interface.outputs[*output].system.is_some())
            .map(|(_, &node)| node)
            .chain(vg.discards().iter().flatten().copied())
            .chain(vg.writes().iter().flatten().copied())
            .collect();
        for (&input, &mask) in &pixel_inputs {
            let varying = &pixel.interface.inputs[input];
            // System inputs come from the rasterizer. Vertex system outputs are roots independently of these reads.
            if varying.system.is_some() {
                continue;
            }
            let output = labelled(&vertex.interface.outputs, &varying.label).ok_or_else(|| {
                format!(
                    "pixel input {}{} has no vertex output",
                    varying.label.name, varying.label.index
                )
            })?;
            for lane in 0..varying.value.width {
                if mask & (1 << lane) != 0 {
                    roots.push(vg.output(output, lane).ok_or_else(|| {
                        format!(
                            "pixel input {}{} lane {lane} has no vertex write",
                            varying.label.name, varying.label.index
                        )
                    })?);
                }
            }
        }
        Ok(Self {
            vertex: inputs(&vg, roots),
            pixel: pixel_inputs,
        })
    }
}
