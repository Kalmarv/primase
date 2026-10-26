//! Chunks worked by hand from `lopcodes.h` and `lundump.c`: every byte below is written out and explained, so the
//! reader is checked against the format and not against code that writes it.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_bytes::Endian;
use primase_lua51::{Call, Chunk, Constant, Op, Value, calls};

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn len(parts: &[&[u8]]) -> usize {
    parts.iter().map(|p| p.len()).sum()
}

/// `ESC Lua`, version 0x51, format 0, then big-endian (flag 0), int 4, size_t 4, instruction 4, number 8, not integral.
const HEADER_BE: &[u8] = &[0x1b, b'L', b'u', b'a', 0x51, 0, 0, 4, 4, 4, 8, 0];
/// The same, little-endian (flag 1).
const HEADER_LE: &[u8] = &[0x1b, b'L', b'u', b'a', 0x51, 0, 1, 4, 4, 4, 8, 0];

// ---- A: `print(1.5, "a")`, the main function of a chunk big-endian, with its source name "main" ----
//
//   source        "main": size_t 5 (with the NUL), m a i n NUL
//   linedefined 0, lastlinedefined 0                          two ints
//   nups 0, numparams 0, is_vararg 2 (ISVARARG), maxstack 3   four bytes
//   code, 5 words:
//     GETGLOBAL R0 K0   op 5, A 0, Bx 0                      = 5                          = 0x00000005
//     LOADK R1 K1       op 1, A 1 << 6, Bx 1 << 14           = 1 + 0x40 + 0x4000          = 0x00004041
//     LOADK R2 K2       op 1, A 2 << 6, Bx 2 << 14           = 1 + 0x80 + 0x8000          = 0x00008081
//     CALL R0 B3 C1     op 28, C 1 << 14, B 3 << 23          = 0x1c + 0x4000 + 0x01800000 = 0x0180401c
//     RETURN R0 B1      op 30, B 1 << 23                     = 0x1e + 0x00800000          = 0x0080001e
//   constants, 3: "print" (type 4, size_t 6, p r i n t NUL), 1.5 (type 3, 0x3ff8000000000000), "a" (type 4, 2, a NUL)
//   nested functions 0, line info 0, locals 0, upvalue names 0
const A_SRC_BE: &[u8] = &[0, 0, 0, 5, b'm', b'a', b'i', b'n', 0];
const A_INTS_BE: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0];
const A_FLAGS: &[u8] = &[0, 0, 2, 3];
const A_CODE_BE: &[u8] = &[
    0, 0, 0, 5, // count
    0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x40, 0x41, 0x00, 0x00, 0x80, 0x81, 0x01, 0x80, 0x40, 0x1c, 0x00,
    0x80, 0x00, 0x1e,
];
const A_CONSTANTS_BE: &[u8] = &[
    0, 0, 0, 3, // count
    4, 0, 0, 0, 6, b'p', b'r', b'i', b'n', b't', 0, // "print"
    3, 0x3f, 0xf8, 0, 0, 0, 0, 0, 0, // 1.5
    4, 0, 0, 0, 2, b'a', 0, // "a"
];
const NO_TAIL_BE: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn a_be() -> Vec<u8> {
    cat(&[
        HEADER_BE,
        A_SRC_BE,
        A_INTS_BE,
        A_FLAGS,
        A_CODE_BE,
        A_CONSTANTS_BE,
        NO_TAIL_BE,
    ])
}

/// A, little-endian: every multi-byte number reversed (the double too).
fn a_le() -> Vec<u8> {
    cat(&[
        HEADER_LE,
        &[5, 0, 0, 0, b'm', b'a', b'i', b'n', 0],
        &[0; 8],
        A_FLAGS,
        &[
            5, 0, 0, 0, // count
            0x05, 0x00, 0x00, 0x00, 0x41, 0x40, 0x00, 0x00, 0x81, 0x80, 0x00, 0x00, 0x1c, 0x40, 0x80, 0x01,
            0x1e, 0x00, 0x80, 0x00,
        ],
        &[
            3, 0, 0, 0, // count
            4, 6, 0, 0, 0, b'p', b'r', b'i', b'n', b't', 0, //
            3, 0, 0, 0, 0, 0, 0, 0xf8, 0x3f, //
            4, 2, 0, 0, 0, b'a', 0,
        ],
        &[0; 16],
    ])
}

fn print_call(c: &Call) {
    assert_eq!(c.callee, Value::Global(b"print".to_vec()));
}

#[test]
fn reads_a_big_endian_function() {
    let c = Chunk::parse(&a_be()).unwrap();
    assert_eq!(c.header.endian, Endian::Big);
    assert_eq!((c.header.int_size, c.header.size_t_size), (4, 4));
    let m = &c.main;
    assert_eq!(m.source.as_deref(), Some(&b"main"[..]));
    assert_eq!((m.upvalues, m.params, m.vararg, m.max_stack), (0, 0, 2, 3));
    let ops: Vec<Op> = m.code.iter().map(|i| i.op).collect();
    assert_eq!(ops, [Op::GetGlobal, Op::LoadK, Op::LoadK, Op::Call, Op::Return]);
    assert_eq!(
        m.constants,
        [
            Constant::Str(b"print".to_vec()),
            Constant::Number(1.5),
            Constant::Str(b"a".to_vec())
        ]
    );
    assert!(m.protos.is_empty() && m.lines.is_empty() && m.locals.is_empty() && m.upvalue_names.is_empty());
}

#[test]
fn little_endian_reads_the_same() {
    let be = Chunk::parse(&a_be()).unwrap();
    let le = Chunk::parse(&a_le()).unwrap();
    assert_eq!(le.header.endian, Endian::Little);
    assert_eq!(le.main, be.main);
}

#[test]
fn lists_the_call_with_its_constant_arguments() {
    let c = Chunk::parse(&a_be()).unwrap();
    let list = calls(&c.main);
    assert_eq!(list.len(), 1);
    print_call(&list[0]);
    assert_eq!(list[0].args, [Value::Number(1.5), Value::Str(b"a".to_vec())]);
    assert_eq!(
        (list[0].pc, list[0].line, list[0].open_args, list[0].tail),
        (3, None, false, false)
    );
    assert!(list[0].path.is_empty());
}

// ---- B: a function that folds, jumps and nests, big-endian, no source name ----
//
//   main (maxstack 3), 12 words:
//     0 GETGLOBAL R0 K0 "a"      0x00000005
//     1 LOADK R1 K1 (10.0)       0x00004041
//     2 UNM R1 R1                op 18 + A 1 << 6 + B 1 << 23 = 0x12 + 0x40 + 0x800000 = 0x00800052
//     3 LOADK R2 K2 "s"          0x00008081
//     4 CALL R0 B3 C1            0x0180401c        a(-10, "s")
//     5 GETGLOBAL R0 K3 "b"      op 5 + Bx 3 << 14 = 0x0000c005
//     6 LOADK R1 K1              0x00004041
//     7 JMP 0                    op 22, Bx 131071 << 14 = 0x7fffc000 + 0x16 = 0x7fffc016 (to instruction 8)
//     8 CALL R0 B2 C1            0x1c + 0x4000 + 2 << 23 = 0x0100401c   a jump lands here: nothing is known
//     9 CLOSURE R0 P0            op 36                    = 0x00000024
//    10 SETGLOBAL R0 K4 "f"      op 7 + Bx 4 << 14        = 0x00010007
//    11 RETURN R0 B1             0x0080001e
//   constants "a", 10.0 (0x4024000000000000), "s", "b", "f"; one nested function; no debug info.
//   nested (maxstack 3), 5 words, `level:Load("p")`, with line info 3 for every instruction:
//     0 GETGLOBAL R0 K0 "level"  0x00000005
//     1 SELF R0 R0 K1 "Load"     op 11 + C (256 + 1) << 14 = 0x0b + 0x404000 = 0x0040400b
//     2 LOADK R2 K2 "p"          0x00008081
//     3 CALL R0 B3 C1            0x0180401c
//     4 RETURN R0 B1             0x0080001e
const NO_SOURCE: &[u8] = &[0, 0, 0, 0];
const B_MAIN_HEAD: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3];
const B_MAIN_CODE: &[u8] = &[
    0, 0, 0, 12, // count
    0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x40, 0x41, 0x00, 0x80, 0x00, 0x52, 0x00, 0x00, 0x80, 0x81, 0x01,
    0x80, 0x40, 0x1c, 0x00, 0x00, 0xc0, 0x05, 0x00, 0x00, 0x40, 0x41, 0x7f, 0xff, 0xc0, 0x16, 0x01, 0x00,
    0x40, 0x1c, 0x00, 0x00, 0x00, 0x24, 0x00, 0x01, 0x00, 0x07, 0x00, 0x80, 0x00, 0x1e,
];
const B_MAIN_CONSTANTS: &[u8] = &[
    0, 0, 0, 5, // count
    4, 0, 0, 0, 2, b'a', 0, //
    3, 0x40, 0x24, 0, 0, 0, 0, 0, 0, //
    4, 0, 0, 0, 2, b's', 0, //
    4, 0, 0, 0, 2, b'b', 0, //
    4, 0, 0, 0, 2, b'f', 0,
];
const B_CHILD_HEAD: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3];
const B_CHILD_CODE: &[u8] = &[
    0, 0, 0, 5, // count
    0x00, 0x00, 0x00, 0x05, 0x00, 0x40, 0x40, 0x0b, 0x00, 0x00, 0x80, 0x81, 0x01, 0x80, 0x40, 0x1c, 0x00,
    0x80, 0x00, 0x1e,
];
const B_CHILD_CONSTANTS: &[u8] = &[
    0, 0, 0, 3, // count
    4, 0, 0, 0, 6, b'l', b'e', b'v', b'e', b'l', 0, //
    4, 0, 0, 0, 5, b'L', b'o', b'a', b'd', 0, //
    4, 0, 0, 0, 2, b'p', 0,
];
/// No nested functions; five line numbers of 3; no locals; no upvalue names.
const B_CHILD_TAIL: &[u8] = &[
    0, 0, 0, 0, // functions
    0, 0, 0, 5, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, 3, // lines
    0, 0, 0, 0, 0, 0, 0, 0,
];
/// One nested function follows the constants; the main function has no debug info.
const B_ONE_FUNCTION: &[u8] = &[0, 0, 0, 1];
const B_MAIN_TAIL: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn b_parts() -> Vec<&'static [u8]> {
    vec![
        HEADER_BE,
        NO_SOURCE,
        B_MAIN_HEAD,
        B_MAIN_CODE,
        B_MAIN_CONSTANTS,
        B_ONE_FUNCTION,
        NO_SOURCE,
        B_CHILD_HEAD,
        B_CHILD_CODE,
        B_CHILD_CONSTANTS,
        B_CHILD_TAIL,
        B_MAIN_TAIL,
    ]
}

#[test]
fn lists_folds_resets_at_jump_targets_and_nested_functions() {
    let b = cat(&b_parts());
    let c = Chunk::parse(&b).unwrap();
    assert_eq!(c.main.source, None);
    assert_eq!(c.main.protos.len(), 1);
    assert_eq!(c.main.protos[0].lines, [3; 5]);
    let list = calls(&c.main);
    assert_eq!(
        list,
        [
            Call {
                path: vec![],
                pc: 4,
                line: None,
                callee: Value::Global(b"a".to_vec()),
                args: vec![Value::Number(-10.0), Value::Str(b"s".to_vec())],
                open_args: false,
                tail: false,
            },
            // the JMP at 7 lands on this CALL, so what the registers held before it is not known
            Call {
                path: vec![],
                pc: 8,
                line: None,
                callee: Value::Unknown,
                args: vec![Value::Unknown],
                open_args: false,
                tail: false,
            },
            Call {
                path: vec![0],
                pc: 3,
                line: Some(3),
                callee: Value::Field(
                    Box::new(Value::Global(b"level".to_vec())),
                    Box::new(Value::Str(b"Load".to_vec()))
                ),
                args: vec![Value::Global(b"level".to_vec()), Value::Str(b"p".to_vec())],
                open_args: false,
                tail: false,
            },
        ]
    );
}

// ---- C: constants of every type, and the tests that need a jump ----
//   two `RETURN R0 B1`; constants nil (type 0), true (type 1, value 1), false (1, 0), maxstack 2.
const C_CODE: &[u8] = &[0, 0, 0, 2, 0x00, 0x80, 0x00, 0x1e, 0x00, 0x80, 0x00, 0x1e];
const C_CONSTANTS: &[u8] = &[0, 0, 0, 3, 0, 1, 1, 1, 0];
const C_HEAD: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];

fn c_parts() -> Vec<&'static [u8]> {
    vec![HEADER_BE, NO_SOURCE, C_HEAD, C_CODE, C_CONSTANTS, &[0; 16]]
}

#[test]
fn reads_nil_and_booleans() {
    let c = Chunk::parse(&cat(&c_parts())).unwrap();
    assert_eq!(
        c.main.constants,
        [Constant::Nil, Constant::Bool(true), Constant::Bool(false)]
    );
}

fn refuses(bytes: &[u8], offset: usize, why: &str) {
    let e = Chunk::parse(bytes).unwrap_err();
    assert_eq!(e.offset, offset, "{e}");
    assert!(e.detail.contains(why), "{e}");
}

fn with(mut bytes: Vec<u8>, at: usize, to: &[u8]) -> Vec<u8> {
    bytes[at..at + to.len()].copy_from_slice(to);
    bytes
}

#[test]
fn refuses_each_header_field() {
    let a = a_be();
    refuses(&with(a.clone(), 0, b"\x1bLub"), 0, "ESC Lua");
    refuses(&with(a.clone(), 4, &[0x52]), 4, "not Lua 5.1");
    refuses(&with(a.clone(), 5, &[1]), 5, "format");
    refuses(&with(a.clone(), 6, &[2]), 6, "byte-order");
    refuses(&with(a.clone(), 7, &[3]), 7, "int");
    refuses(&with(a.clone(), 8, &[2]), 8, "size_t");
    refuses(&with(a.clone(), 9, &[8]), 9, "instruction");
    refuses(&with(a.clone(), 10, &[4]), 10, "8-byte floats");
    refuses(&with(a, 11, &[1]), 10, "8-byte floats");
}

#[test]
fn refuses_every_truncation_and_a_trailing_byte() {
    for bytes in [a_be(), a_le(), cat(&b_parts()), cat(&c_parts())] {
        for n in 0..bytes.len() {
            let e = Chunk::parse(&bytes[..n]).unwrap_err();
            assert!(e.offset <= n, "{e} for {n} bytes");
        }
        let mut more = bytes.clone();
        more.push(0);
        refuses(&more, bytes.len(), "after the main function");
    }
}

#[test]
fn refuses_what_luac_never_writes() {
    let a = a_be();
    // the string table after the header: source at 12, ints at 21, flags at 29, code count at 33, code at 37
    refuses(&with(a.clone(), 20, b"x"), 12, "terminating NUL");
    refuses(&with(a.clone(), 31, &[8]), 31, "is_vararg");
    refuses(&with(a.clone(), 32, &[251]), 32, "251 registers");
    refuses(&with(a.clone(), 33, &[0xff, 0xff, 0xff, 0xff]), 33, "count");
    // constants: count at 57; "print" starts at 61; 1.5's type byte is at 61 + 11 = 72
    refuses(&with(a.clone(), 72, &[2]), 72, "type 2");
    let c = cat(&c_parts());
    // C's constants: the count at 12 + 4 + 12 + 12 = 40, nil's type at 44, true's type at 45 and value at 46
    refuses(&with(c, 46, &[2]), 46, "boolean");
}

#[test]
fn verifies_the_code() {
    let a = a_be();
    refuses(&with(a.clone(), 37, &[0, 0, 0, 63]), 37, "opcode 63");
    refuses(&with(a.clone(), 53, &[0, 0, 0, 0]), 53, "end in RETURN");
    refuses(&with(a.clone(), 32, &[2]), 45, "past the stack");
    refuses(&with(a.clone(), 41, &[0x00, 0x02, 0x40, 0x41]), 41, "constant 9");
    // a SELF/GETTABLE style operand: LOADK's A past the stack was above; GETGLOBAL's constant past the table:
    refuses(&with(a, 37, &[0x00, 0x00, 0xc0, 0x05]), 37, "constant 3");

    let b = cat(&b_parts());
    let jmp = len(&b_parts()[..4]) - B_MAIN_CODE.len() + 4 + 7 * 4;
    assert_eq!(&b[jmp..jmp + 4], &[0x7f, 0xff, 0xc0, 0x16]);
    refuses(
        &with(b.clone(), jmp, &[0x80, 0x18, 0xc0, 0x16]),
        jmp,
        "jump to 108",
    );
    // the CLOSURE at 9 names a function that captures one upvalue, which no instruction after it supplies
    let child_ups = len(&b_parts()[..7]);
    refuses(
        &with(b, child_ups + 8, &[1]),
        len(&b_parts()[..4]) - B_MAIN_CODE.len() + 4 + 9 * 4,
        "CLOSURE",
    );

    // TEST R0 C0 (op 26) followed by RETURN, not by a jump
    let c = cat(&c_parts());
    let code = len(&c_parts()[..3]) + 4;
    refuses(&with(c, code, &[0, 0, 0, 0x1a]), code, "not followed by a jump");
}

/// Functions nested 200 deep read; 201 are refused at the 201st.
#[test]
fn bounds_nesting() {
    // one level: no source, ints, flags (maxstack 2), `RETURN`, no constants, N functions, then 3 empty tables
    let head: &[u8] = &[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0x80, 0, 0x1e, 0, 0, 0, 0,
    ];
    let tail: &[u8] = &[0; 12];
    let nest = |depth: usize| {
        let mut bytes = HEADER_BE.to_vec();
        for _ in 0..=depth {
            bytes.extend_from_slice(head);
            bytes.extend_from_slice(&[0, 0, 0, 1]);
        }
        // the innermost has no function: replace its count of 1 by 0
        let n = bytes.len();
        bytes[n - 1] = 0;
        for _ in 0..=depth {
            bytes.extend_from_slice(tail);
        }
        bytes
    };
    Chunk::parse(&nest(199)).unwrap();
    let e = Chunk::parse(&nest(200)).unwrap_err();
    assert!(e.detail.contains("nested past 200"), "{e}");
}
