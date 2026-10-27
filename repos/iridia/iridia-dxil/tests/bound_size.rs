#![allow(clippy::unwrap_used)]
//! The bound size of a constant buffer ([`iridia::BoundSizes`]): a program that declares a palette of two matrices
//! (six rows) and indexes it by a joint reads zero past the declaration by default, and the buffer's own rows up to the
//! size the source binds when the lift is given it, as D3D12 bounds a constant buffer read by the view and not by the
//! declaration (`fixtures/bound_palette_vs.hlsl`).
use iridia::{BoundSizes, Program, ResourceId, ResourceKind, SystemValue};
use iridia_exec::{Backend, Element, Executable, ImageLoad, ImageQuery, ImageSample, Resources, Runner};

const PROGRAM: &[u8] = include_bytes!("fixtures/bound_palette_vs.dxil");

/// A backend that answers every read with zero.
struct Zeros;
impl Backend for Zeros {
    fn sample(&mut self, _request: ImageSample<Element>) -> Result<[f32; 4], String> {
        Ok([0.0; 4])
    }
    fn read_buffer(&mut self, _view: ResourceId, _word: u32, _count: u8) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
    fn load(&mut self, _request: ImageLoad<Element>) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
    fn query(&mut self, _request: ImageQuery<Element>) -> Result<[u32; 4], String> {
        Ok([1; 4])
    }
    fn buffer_words(&mut self, _view: ResourceId) -> Result<u32, String> {
        Ok(1 << 20)
    }
}

/// The bytes of the palette buffer the lift declares.
fn palette_bytes(program: &Program) -> u32 {
    program
        .interface
        .resources
        .iter()
        .find_map(|r| match &r.kind {
            ResourceKind::ConstantBuffer { layout } => Some(layout.size),
            _ => None,
        })
        .unwrap()
}

/// The first three lanes of the clip position for the joint `joint`, with row `r` of the palette four lanes of `r`
/// (so the position `(0, 0, 0)` gives the row's number): the program's rows `3 * joint` to `3 * joint + 2`, zero where it
/// reads past what the buffer holds.
fn rows_of_joint(program: &Program, joint: u32) -> [f32; 3] {
    let iface = &program.interface;
    let position = iface
        .outputs
        .iter()
        .position(|o| o.system == Some(SystemValue::ClipPosition))
        .unwrap();
    let exe = Executable::new(program).unwrap();
    let mut resources = Resources::default();
    for r in &iface.resources {
        let ResourceKind::ConstantBuffer { layout } = &r.kind else {
            continue;
        };
        let rows = (0..layout.size.div_ceil(16))
            .map(|row| [(row as f32).to_bits(); 4])
            .collect();
        resources.constants.insert(r.id, rows);
    }
    let bound = exe.bind_resources(&resources).unwrap();
    let args: Vec<[u32; 4]> = iface
        .inputs
        .iter()
        .map(|i| {
            if i.label.name.eq_ignore_ascii_case("BLENDINDICES") {
                [joint, 0, 0, 0]
            } else {
                [0; 4]
            }
        })
        .collect();
    let mut backend = Zeros;
    let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
    let [out] = runner.run([&args]).unwrap();
    let clip = out.values[position].map(f32::from_bits);
    [clip[0], clip[1], clip[2]]
}

fn palette_id(program: &Program) -> ResourceId {
    program
        .interface
        .resources
        .iter()
        .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
        .unwrap()
        .id
}

#[test]
fn a_read_past_the_declaration_is_zero_and_inside_the_bound_is_the_buffers() {
    let declared = iridia_dxil::lift(PROGRAM).unwrap();
    assert_eq!(palette_bytes(&declared), 96);
    // joint 1 is rows 3 to 5, inside the declaration; joint 2 is rows 6 to 8, past it
    assert_eq!(rows_of_joint(&declared, 1), [3.0, 4.0, 5.0]);
    assert_eq!(rows_of_joint(&declared, 2), [0.0; 3]);

    // bound at 256 matrices of three rows: 768 rows
    let mut bound = BoundSizes::new();
    bound.set(palette_id(&declared), 12_288);
    let wide = iridia_dxil::lift_with(PROGRAM, &bound).unwrap();
    assert_eq!(palette_bytes(&wide), 12_288);
    assert_eq!(rows_of_joint(&wide, 1), [3.0, 4.0, 5.0]);
    assert_eq!(rows_of_joint(&wide, 2), [6.0, 7.0, 8.0]);
    assert_eq!(rows_of_joint(&wide, 255), [765.0, 766.0, 767.0]);
    // past the bound it is zero again
    assert_eq!(rows_of_joint(&wide, 256), [0.0; 3]);
    // an empty set of sizes is the plain lift
    assert_eq!(
        iridia_dxil::lift_with(PROGRAM, &BoundSizes::new())
            .unwrap()
            .interface,
        declared.interface
    );
}

#[test]
fn a_bound_below_the_declaration_or_for_another_resource_is_refused() {
    let declared = iridia_dxil::lift(PROGRAM).unwrap();
    let mut below = BoundSizes::new();
    below.set(palette_id(&declared), 48);
    let err = iridia_dxil::lift_with(PROGRAM, &below).unwrap_err().to_string();
    assert!(err.contains("below the 96 its program declares"), "{err}");
    let mut missing = BoundSizes::new();
    missing.set(ResourceId(99), 1024);
    let err = iridia_dxil::lift_with(PROGRAM, &missing).unwrap_err().to_string();
    assert!(err.contains("the program does not have"), "{err}");
}
