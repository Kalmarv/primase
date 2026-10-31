//! The light-record helpers the games' light-table tests share: the capture proxies' program hash, records as floats,
//! the light tables a render graph's organize step binds, and a packed record against a recorded one, field by field.
use primase_core::Name;
use primase_core::World;
use primase_core::render_graph::{PassWork, RenderPass};
use primase_core::shader::{BufferSource, ConstantBinding, LightTable, LightTableKind, LightValue};
use std::collections::BTreeMap;

/// The capture proxies' hash of a program's bytes: FNV-1a 64 from the seed 1469598103934665603.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// The light tables the graph's step of the program whose name ends with `program` binds, by kind, and its constants.
pub fn organize_tables(
    passes: &[RenderPass],
    world: &World,
    program: &str,
    out: &mut BTreeMap<&'static str, LightTable>,
    constants: &mut Vec<ConstantBinding>,
) {
    for p in passes {
        match &p.work {
            PassWork::Compute { stage, .. }
                if world
                    .get::<Name>(stage.shader)
                    .is_some_and(|n| n.0.ends_with(program)) =>
            {
                constants.extend(stage.constants.iter().cloned());
                for b in &stage.buffers {
                    if let BufferSource::Lights(t) = &b.source {
                        let kind = match t.kind {
                            LightTableKind::Point => "point",
                            LightTableKind::Spot => "spot",
                        };
                        out.insert(kind, t.clone());
                    }
                }
            }
            PassWork::Group { steps } => organize_tables(steps, world, program, out, constants),
            _ => {}
        }
    }
}

/// How a packed field compares with the recorded one: per `<kind> <field>`, the matched and the differing records.
#[derive(Default)]
pub struct Tally(pub BTreeMap<String, [usize; 2]>);
impl Tally {
    pub fn add(&mut self, key: String, same: bool) {
        self.0.entry(key).or_default()[usize::from(!same)] += 1;
    }
    pub fn get(&self, key: &str) -> [usize; 2] {
        self.0.get(key).copied().unwrap_or_default()
    }
}

pub fn near(a: f32, b: f32, abs: f64) -> bool {
    (f64::from(a) - f64::from(b)).abs() <= abs
}

pub fn relative(a: f32, b: f32, rel: f64) -> bool {
    (f64::from(a) - f64::from(b)).abs() <= rel * f64::from(b).abs().max(1e-9)
}

/// Each field of `table` in a packed record against the recorded one it stands on.
pub fn compare(kind: &str, table: &LightTable, packed: &[f32], recorded: &[f32], tally: &mut Tally) {
    for f in &table.fields {
        let at = f.offset as usize / 4;
        let n = f.value.floats() as usize;
        let (p, r) = (&packed[at..at + n], &recorded[at..at + n]);
        let name = |what: &str| format!("{kind} {what} at {}", f.offset);
        match &f.value {
            LightValue::ViewPosition | LightValue::ViewDepthMin | LightValue::ViewDepthMax => {
                tally.add(name("view"), p.iter().zip(r).all(|(a, b)| near(*a, *b, 1e-3)));
            }
            LightValue::Range | LightValue::InverseRangeSquared => {
                tally.add(name("range"), relative(p[0], r[0], 1e-5));
            }
            LightValue::InverseFalloffDistance => {
                tally.add(name("inverse falloff distance"), p[0].to_bits() == r[0].to_bits());
            }
            LightValue::TranslucencyStrength | LightValue::TranslucencyExponentScale => {
                tally.add(name("translucency"), p[0].to_bits() == r[0].to_bits());
            }
            LightValue::Constant { .. } => tally.add(name("constant"), p == r),
            LightValue::Colour | LightValue::SpecularColour => {
                let exact = p.iter().zip(r).all(|(a, b)| relative(*b, *a, 1e-5));
                // the recorded colour the packed one times one factor below 1, the same in every channel
                let factors: Vec<f64> = p
                    .iter()
                    .zip(r)
                    .filter(|(a, _)| **a > 0.0)
                    .map(|(a, b)| f64::from(*b) / f64::from(*a))
                    .collect();
                let scaled = !exact
                    && !factors.is_empty()
                    && factors
                        .iter()
                        .all(|k| *k < 1.0 && (k - factors[0]).abs() <= 1e-5 * factors[0]);
                tally.add(name("colour exact"), exact);
                tally.add(name("colour scaled"), scaled);
            }
            LightValue::ViewToCone => {
                for v in 0..4 {
                    let (pv, rv) = (&p[4 * v..4 * v + 4], &r[4 * v..4 * v + 4]);
                    let scale = rv.iter().fold(0f32, |m, x| m.max(x.abs())) as f64;
                    tally.add(
                        name(&format!("cone vector {v}")),
                        pv.iter()
                            .zip(rv)
                            .all(|(a, b)| near(*a, *b, 1e-4 * scale.max(1.0))),
                    );
                }
            }
            LightValue::ViewBoundingSphere => {
                tally.add(name("sphere"), p.iter().zip(r).all(|(a, b)| near(*a, *b, 1e-3)));
            }
            // the shadowed variants' fields: the frames hold no shadowed light, every one is 0
            LightValue::Unknown { name: field, why, .. } if why.contains("Sdw") => {
                tally.add(format!("{kind} {field} zero"), r.iter().all(|x| *x == 0.0));
            }
            _ => {}
        }
    }
}
