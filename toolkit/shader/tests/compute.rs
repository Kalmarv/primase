#![allow(clippy::unwrap_used)]
//! A compute program built by hand: a thread group of four, a thread-group counter every invocation adds its index to,
//! a barrier, and one invocation writing the sum to a read-write buffer. It validates, is written as WGSL, and a group
//! D3D cannot have is refused.
use std::num::NonZeroU32;

use naga::{
    AddressSpace, AtomicFunction, Barrier, Expression, GlobalVariable, ResourceBinding, Scalar, Statement,
    StorageAccess, TypeInner,
};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::{
    Access, BufferKind, Conventions, D3dBinding, D3dClass, D3dModes, Interface, Label, Program, Provenance,
    Resource, ResourceId, ResourceKind, ResourceProvenance, ScalarType, ShaderFormat, Stage, StorageTarget,
    SystemValue, Value, Varying, VaryingProvenance,
};

fn program(size: [u32; 3]) -> Program {
    let mut b = Builder::new();
    let u1 = b.ty_of(Kind::U, 1);
    let atomic = b.ty(TypeInner::Atomic(Scalar::U32));
    let shared_ty = b.ty(TypeInner::Array {
        base: atomic,
        size: naga::ArraySize::Constant(NonZeroU32::new(1).unwrap()),
        stride: 4,
    });
    let shared = b.global(GlobalVariable {
        name: Some("counter".into()),
        space: AddressSpace::WorkGroup,
        binding: None,
        ty: shared_ty,
        init: None,
        memory_decorations: Default::default(),
    });
    let words_ty = b.ty(TypeInner::Array {
        base: u1,
        size: naga::ArraySize::Dynamic,
        stride: 4,
    });
    let words = b.global(GlobalVariable {
        name: Some("u0".into()),
        space: AddressSpace::Storage {
            access: StorageAccess::LOAD | StorageAccess::STORE,
        },
        binding: Some(ResourceBinding { group: 3, binding: 0 }),
        ty: words_ty,
        init: None,
        memory_decorations: Default::default(),
    });

    let index_ty = u1;
    let index = b.app(Expression::FunctionArgument(0));
    let zero = b.lit_u(0);
    // counter[0] += index
    let shared_ptr = b.global_ptr(shared);
    let slot = b.app(Expression::AccessIndex {
        base: shared_ptr,
        index: 0,
    });
    let old_ty = b.ty_of(Kind::U, 1);
    let old = b.result_expr(Expression::AtomicResult {
        ty: old_ty,
        comparison: false,
    });
    b.push(Statement::Atomic {
        pointer: slot,
        fun: AtomicFunction::Add,
        value: index,
        result: Some(old),
    });
    b.push(Statement::ControlBarrier(Barrier::WORK_GROUP));
    // if index == 0 { words[0] = counter[0] }
    let is_first = b.binary(
        naga::BinaryOperator::Equal,
        Val {
            expr: index,
            kind: Kind::U,
            width: 1,
        },
        zero,
    );
    b.when(is_first, |b| {
        let shared_ptr = b.global_ptr(shared);
        let slot = b.app(Expression::AccessIndex {
            base: shared_ptr,
            index: 0,
        });
        let total = b.load(slot, Kind::U, 1);
        let words_ptr = b.global_ptr(words);
        let zero = b.lit_u(0);
        let target = b.app(Expression::Access {
            base: words_ptr,
            index: zero.expr,
        });
        b.store(target, total);
    });
    b.workgroup_size = size;
    let module = b.finish(
        "main",
        naga::ShaderStage::Compute,
        vec![naga::FunctionArgument {
            name: Some("SV_GroupIndex0".into()),
            ty: index_ty,
            binding: Some(naga::Binding::BuiltIn(naga::BuiltIn::LocalInvocationIndex)),
        }],
        None,
        false,
    );
    let words = module.global_variables.iter().nth(1).unwrap().0;
    Program {
        module,
        stage: Stage::Compute,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Compute,
            inputs: vec![Varying {
                label: Label {
                    name: "SV_GroupIndex".into(),
                    index: 0,
                },
                system: Some(SystemValue::GroupIndex),
                value: Value {
                    scalar: ScalarType::Uint,
                    width: 1,
                },
                interp: None,
                used: true,
                provenance: VaryingProvenance::default(),
            }],
            outputs: Vec::new(),
            resources: vec![Resource {
                id: ResourceId(0),
                kind: ResourceKind::Storage {
                    target: StorageTarget::Buffer(BufferKind::Raw),
                    access: Access::ReadWrite,
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: "sums".into(),
                    d3d: Some(D3dBinding {
                        class: D3dClass::UnorderedAccess,
                        register: 0,
                        space: 0,
                    }),
                },
            }],
            facts: Default::default(),
        },
        bindings: vec![(ResourceId(0), words)],
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: Provenance {
            format: ShaderFormat::Dxil,
            target: "cs_6_0".into(),
            lifter: "test".into(),
            lifter_version: 0,
            source_hash: None,
        },
        notes: Vec::new(),
    }
}

#[test]
fn a_compute_program_validates_and_is_written_as_wgsl() {
    let p = program([4, 1, 1]);
    p.validate().unwrap_or_else(|e| panic!("{e:?}"));
    let wgsl = primase_shader::emit::wgsl(&p).unwrap();
    for needle in [
        "@compute @workgroup_size(4, 1, 1)",
        "var<workgroup> counter: array<atomic<u32>, 1>",
        "atomicAdd(",
        "workgroupBarrier()",
        "@builtin(local_invocation_index)",
        "var<storage, read_write> u0_: array<u32>",
    ] {
        assert!(wgsl.contains(needle), "{needle:?} is not in\n{wgsl}");
    }
    assert!(p.text().unwrap().contains("// stage compute entry main"));
}

#[test]
fn a_thread_group_d3d_cannot_have_is_refused() {
    for size in [[0, 1, 1], [1025, 1, 1], [1, 1, 65], [32, 32, 2]] {
        let problems = program(size).validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.to_string().contains("a thread group of")),
            "{size:?}: {problems:?}"
        );
    }
}

#[test]
fn a_thread_group_beyond_webgpus_defaults_says_so() {
    let notes = primase_shader::compute_limit_notes(&program([4, 1, 1]).module);
    assert!(notes.is_empty(), "{notes:?}");
    let notes = primase_shader::compute_limit_notes(&program([64, 8, 1]).module);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("64 by 8 by 1 (512 invocations)"), "{notes:?}");
    let notes = primase_shader::compute_limit_notes(&program([512, 1, 1]).module);
    assert!(notes[0].contains("512 by 1 by 1"), "{notes:?}");
}
