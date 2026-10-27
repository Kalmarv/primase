//! What the builder does when a lifter hands it values that cannot do what it is asked: the fault waits for the lifter to
//! ask for it, and the builder is never left to panic on a program's account.
use naga::{BinaryOperator, MathFunction};
use iridia::build::{Builder, Kind};

fn vector(b: &mut Builder, width: usize) -> iridia::build::Val {
    let one = b.lit_f(1.0);
    b.compose(&vec![one; width])
}

#[test]
fn an_operation_on_values_of_other_shapes_is_a_fault_the_lifter_asks_for() {
    let mut b = Builder::new();
    assert!(b.check().is_ok());
    let (two, three) = (vector(&mut b, 2), vector(&mut b, 3));
    b.binary(BinaryOperator::Add, two, three);
    let fault = b.check().unwrap_err();
    assert!(fault.contains("2 lanes where 3 are needed"), "{fault}");
    // asking takes it
    assert!(b.check().is_ok());
    // the first of several is the one kept
    b.lane(two, 2);
    b.swizzle(three, &[0, 3]);
    let fault = b.check().unwrap_err();
    assert!(fault.contains("lane 2 of a value of 2 lanes"), "{fault}");
    assert!(b.check().is_ok());
}

#[test]
fn a_vector_of_no_lanes_or_too_many_and_a_function_of_no_arguments_are_faults() {
    let mut b = Builder::new();
    b.compose(&[]);
    assert!(b.check().unwrap_err().contains("no lanes"));
    let one = b.lit_f(1.0);
    b.compose(&[one; 5]);
    assert!(b.check().unwrap_err().contains("5 parts"));
    let word = b.lit_u(1);
    b.compose(&[one, word]);
    assert!(b.check().unwrap_err().contains("scalars of one kind"));
    b.math(MathFunction::Abs, &[]);
    assert!(b.check().unwrap_err().contains("no arguments"));
    b.swizzle(one, &[]);
    assert!(b.check().is_err());
    assert_eq!(b.ty_of(Kind::F, 1), b.ty_of(Kind::F, 1));
}

#[test]
#[should_panic(expected = "without checking it")]
fn a_builder_with_a_fault_nobody_asked_for_cannot_be_finished() {
    let mut b = Builder::new();
    let (two, three) = (vector(&mut b, 2), vector(&mut b, 3));
    b.binary(BinaryOperator::Add, two, three);
    b.finish_functions();
}
