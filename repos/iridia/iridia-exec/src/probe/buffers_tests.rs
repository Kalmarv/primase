use super::*;
use crate::probe::AnsweredReads;
use crate::{AtomicAdd, PixelShader, ProgramShader, Store};

// Image defaults must not supply undeclared structured views or enable writes in a read-only probe.
struct PermissiveImages;
impl Sampler for PermissiveImages {
    fn sample(&mut self, _: Sample) -> Result<[f32; 4], String> {
        Ok([2., 3., 4., 5.])
    }
    fn buffer_elements(&mut self, _: u32) -> Option<u32> {
        Some(0)
    }
    fn store(&mut self, _: Store) -> Result<(), String> {
        Ok(())
    }
    fn atomic_add(&mut self, _: AtomicAdd) -> Result<u32, String> {
        Ok(0)
    }
}

fn extents() -> BTreeMap<u32, BufferExtent> {
    BTreeMap::from([(
        3,
        BufferExtent {
            stride: 20,
            elements: 2,
        },
    )])
}

fn word(request: BufferRead) -> Result<u32, String> {
    assert_eq!(request.slot, 3);
    // The lifted bounds guard may load valid element zero before selecting zero for an out-of-range access.
    let values = match request.element {
        0 => [10.0f32, 20., 30., 40., 50.],
        1 => [0.2f32, 0.4, 0.6, 0.8, 0.25],
        _ => panic!("a request beyond the stated extent: {request:?}"),
    };
    Ok(values[request.offset as usize / 4].to_bits())
}

fn images() -> AnsweredReads<u32, impl super::super::Answer<u32>> {
    AnsweredReads::new(|r: &ImageSample<u32>| {
        assert_eq!(r.texture, 1);
        assert_eq!(&r.coord[..2], &[0.25, 0.75]);
        Ok([2., 3., 4., 5.])
    })
}

/// Both compilers use the same adapter: a 20-byte structured element and the image answer reach the actual output.
/// A stated out-of-range element reads zero; an undeclared view refuses instead of inventing a zero light table.
#[test]
fn compiled_programs_read_answered_buffers_and_images_together() {
    let programs = [
        (
            "fxc",
            iridia_dxbc::lift(include_bytes!("../../tests/fixtures/answered_buffers.dxbc")).unwrap(),
        ),
        (
            "dxc",
            primase_dxil::lift(include_bytes!("../../tests/fixtures/answered_buffers.dxil")).unwrap(),
        ),
    ];
    for (compiler, program) in programs {
        let shader = ProgramShader::new(program).unwrap();
        let inputs = vec![[0.25f32.to_bits(), 0.75f32.to_bits(), 0, 0]; shader.input_count()];
        let mut answers = AnsweredBuffers::new(images(), extents(), word).unwrap();
        let output = shader
            .execute(&inputs, &[(0, &[[1, 0, 0, 0]])], &mut answers)
            .unwrap();
        for (got, want) in output.outputs[0]
            .map(f32::from_bits)
            .into_iter()
            .zip([0.65, 1.45, 2.65, 4.25])
        {
            assert!((got - want).abs() < 1e-6, "{compiler}: {got} != {want}");
        }
        assert!(!output.discarded);
        assert_eq!(answers.images.samples.len(), 1);
        // The scale's DXIL bufferLoad has four result lanes. Guards may load element zero for its three unused
        // lanes beyond the element's stride. Check the five meaningful answers and the result, not a compiler's
        // exact intermediate read count.
        for offset in [0, 4, 8, 12, 16] {
            let request = BufferRead {
                slot: 3,
                element: 1,
                offset,
            };
            assert!(answers.reads.contains(&(request, word(request).unwrap())));
        }
        let zero = shader
            .execute(&inputs, &[(0, &[[2, 0, 0, 0]])], &mut answers)
            .unwrap();
        assert_eq!(zero.outputs[0].map(f32::from_bits), [0.; 4]);
        assert!(answers.queries.contains(&(3, Some(2))));

        let mut missing = AnsweredBuffers::new(images(), BTreeMap::new(), word).unwrap();
        let error = shader
            .execute(&inputs, &[(0, &[[1, 0, 0, 0]])], &mut missing)
            .unwrap_err();
        assert!(
            error.to_string().contains("t3: no structured extent stated"),
            "{error}"
        );
        assert!(missing.reads.is_empty());
    }
}

#[test]
fn invalid_extents_and_addresses_refuse_before_the_answer_is_called() {
    for extent in [
        BufferExtent {
            stride: 0,
            elements: 1,
        },
        BufferExtent {
            stride: 6,
            elements: 1,
        },
        BufferExtent {
            stride: 8,
            elements: u32::MAX,
        },
    ] {
        assert!(AnsweredBuffers::new(images(), BTreeMap::from([(3, extent)]), word).is_err());
    }
    let mut answers = AnsweredBuffers::new(images(), extents(), |_: BufferRead| -> Result<u32, String> {
        panic!("an invalid or out-of-range request must not call the answer")
    })
    .unwrap();
    assert!(
        answers
            .structured(4, 0, 0)
            .unwrap_err()
            .contains("no structured extent")
    );
    for offset in [1, 20, u32::MAX] {
        assert!(
            answers
                .structured(3, 0, offset)
                .unwrap_err()
                .contains("byte offset")
        );
    }
    assert!(answers.reads.is_empty());
    assert_eq!(answers.structured(3, 2, 0), Ok(0));
    assert_eq!(
        answers.reads,
        [(
            BufferRead {
                slot: 3,
                element: 2,
                offset: 0
            },
            0
        )]
    );
}

#[test]
fn a_missing_word_keeps_its_reason_and_writes_remain_refused() {
    let mut answers = AnsweredBuffers::new(PermissiveImages, extents(), |_: BufferRead| {
        Err("not established".into())
    })
    .unwrap();
    assert_eq!(
        answers.structured(3, 1, 4),
        Err("t3: element 1 byte 4: not established".into())
    );
    assert!(answers.reads.is_empty());
    assert!(
        answers
            .store(Store {
                view: 3,
                address: [0; 3],
                value: [0; 4],
                mask: 15
            })
            .is_err()
    );
    assert!(
        answers
            .atomic_add(AtomicAdd {
                view: 3,
                address: [0; 3],
                value: 1
            })
            .is_err()
    );
}

#[test]
fn an_image_default_cannot_hide_a_missing_structured_view() {
    let program = iridia_dxbc::lift(include_bytes!("../../tests/fixtures/answered_buffers.dxbc")).unwrap();
    let shader = ProgramShader::new(program).unwrap();
    let inputs = vec![[0; 4]; shader.input_count()];
    let mut answers = AnsweredBuffers::new(PermissiveImages, BTreeMap::new(), word).unwrap();
    assert_eq!(answers.buffer_elements(3), None);
    let error = shader
        .execute(&inputs, &[(0, &[[0; 4]])], &mut answers)
        .unwrap_err();
    assert!(
        error.to_string().contains("no structured extent stated"),
        "{error}"
    );
    assert!(answers.reads.is_empty());
}
