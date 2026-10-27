//! A level script written out byte by byte (little-endian Lua 5.1, as the game's scripts are) inside a hand-framed
//! PAK: the calls with literal arguments are read in order, a call with a variable argument is reported, a script with
//! control flow is not read, and a missing script is a report line, not an error.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::{
    convert::{self, Reader},
    frame::SourceFrame,
};
use primase_shred::{
    components::{LevelParams, ScriptCall, ScriptValue},
    read::{
        convert::ShredReader,
        level_params::{ScriptInputs, read_level_params},
        pak::Pak,
    },
};

/// `ESC Lua`, 0x51, format 0, little-endian (1), int 4, size_t 4, instruction 4, number 8, not integral.
const HEADER: &[u8] = &[0x1b, b'L', b'u', b'a', 0x51, 0, 1, 4, 4, 4, 8, 0];
/// No source name; line defined 0, last line defined 0.
const MAIN_HEAD: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// No nested functions, line info, locals or upvalue names.
const TAIL: &[u8] = &[0; 16];

/// `change_fog(400, 700)  enable_fog()  set_back_light(0.5, "x", true, nil)  mystery(somevar)`, maxstack 5:
///   0 GETGLOBAL R0 K0          op 5                                      0x00000005
///   1 LOADK R1 K1              op 1 + A 1 << 6 + Bx 1 << 14              0x00004041
///   2 LOADK R2 K2              1 + 2 << 6 + 2 << 14                      0x00008081
///   3 CALL R0 B3 C1            op 28 + C 1 << 14 + B 3 << 23             0x0180401c
///   4 GETGLOBAL R0 K3          5 + 3 << 14                               0x0000c005
///   5 CALL R0 B1 C1            28 + 1 << 14 + 1 << 23                    0x0080401c
///   6 GETGLOBAL R0 K4          5 + 4 << 14                               0x00010005
///   7 LOADK R1 K5              1 + 1 << 6 + 5 << 14                      0x00014041
///   8 LOADK R2 K6              1 + 2 << 6 + 6 << 14                      0x00018081
///   9 LOADBOOL R3 1 0          op 2 + 3 << 6 + B 1 << 23                 0x008000c2
///  10 LOADNIL R4 R4            op 3 + 4 << 6 + B 4 << 23                 0x02000103
///  11 CALL R0 B5 C1            28 + 1 << 14 + 5 << 23                    0x0280401c
///  12 GETGLOBAL R0 K7          5 + 7 << 14                               0x0001c005
///  13 GETGLOBAL R1 K8          5 + 1 << 6 + 8 << 14                      0x00020045
///  14 CALL R0 B2 C1            28 + 1 << 14 + 2 << 23                    0x0100401c
///  15 RETURN R0 B1             op 30 + 1 << 23                           0x0080001e
fn script() -> Vec<u8> {
    let code: &[u8] = &[
        16, 0, 0, 0, // count
        0x05, 0x00, 0x00, 0x00, 0x41, 0x40, 0x00, 0x00, 0x81, 0x80, 0x00, 0x00, 0x1c, 0x40, 0x80,
        0x01, //
        0x05, 0xc0, 0x00, 0x00, 0x1c, 0x40, 0x80, 0x00, 0x05, 0x00, 0x01, 0x00, 0x41, 0x40, 0x01,
        0x00, //
        0x81, 0x80, 0x01, 0x00, 0xc2, 0x00, 0x80, 0x00, 0x03, 0x01, 0x00, 0x02, 0x1c, 0x40, 0x80,
        0x02, //
        0x05, 0xc0, 0x01, 0x00, 0x45, 0x00, 0x02, 0x00, 0x1c, 0x40, 0x00, 0x01, 0x1e, 0x00, 0x80, 0x00,
    ];
    // Nine constants: names are type 4, a size with the NUL, the bytes and the NUL; numbers are type 3 and a
    // little-endian double (400 = 0x4079000000000000, 700 = 0x4085e00000000000, 0.5 = 0x3fe0000000000000).
    let constants: &[&[u8]] = &[
        &[9, 0, 0, 0],
        &[4, 11, 0, 0, 0],
        b"change_fog\0",
        &[3, 0, 0, 0, 0, 0, 0, 0x79, 0x40],
        &[3, 0, 0, 0, 0, 0, 0xe0, 0x85, 0x40],
        &[4, 11, 0, 0, 0],
        b"enable_fog\0",
        &[4, 15, 0, 0, 0],
        b"set_back_light\0",
        &[3, 0, 0, 0, 0, 0, 0, 0xe0, 0x3f],
        &[4, 2, 0, 0, 0],
        b"x\0",
        &[4, 8, 0, 0, 0],
        b"mystery\0",
        &[4, 8, 0, 0, 0],
        b"somevar\0",
    ];
    [HEADER, MAIN_HEAD, &[0, 0, 2, 5], code, &constants.concat(), TAIL].concat()
}

/// `JMP 0` then `RETURN`: control flow (op 22, Bx 131071 << 14 = 0x7fffc000).
fn branching_script() -> Vec<u8> {
    let code: &[u8] = &[2, 0, 0, 0, 0x16, 0xc0, 0xff, 0x7f, 0x1e, 0x00, 0x80, 0x00];
    [HEADER, MAIN_HEAD, &[0, 0, 2, 2], code, &[0, 0, 0, 0], TAIL].concat()
}

/// A PAK of version 4 holding stored entries `(name, payload)`, no sections.
fn pak(entries: &[(&str, &[u8])]) -> Pak {
    let mut names = Vec::new();
    let mut offsets = Vec::new();
    for (n, _) in entries {
        offsets.push(names.len() as u32);
        names.extend(n.as_bytes());
        names.push(0);
    }
    let mut b = b"PAK\0".to_vec();
    b.extend(0xfeebu16.to_be_bytes());
    b.extend(4u16.to_be_bytes());
    b.extend((entries.len() as u32).to_be_bytes());
    b.extend((names.len() as u32).to_be_bytes());
    let mut at = 16 + 32 * entries.len() as u32 + names.len() as u32;
    for ((n, payload), off) in entries.iter().zip(&offsets) {
        for w in [
            primase_foundations::hash::sdbm(n.as_bytes()),
            *off,
            0,
            0,
            at,
            payload.len() as u32,
            0,
            0,
        ] {
            b.extend(w.to_be_bytes());
        }
        at += payload.len() as u32;
    }
    b.extend(&names);
    for (_, payload) in entries {
        b.extend(*payload);
    }
    Pak::from_bytes(b).unwrap()
}

#[test]
fn reads_literal_calls_in_order_and_reports_the_rest() {
    let pak = pak(&[("script\\vfx_level_load_zulu.lua", &script())]);
    let (params, report) = read_level_params(&ScriptInputs {
        scripts: &pak,
        level: "zulu",
    })
    .unwrap();
    let number = |n| ScriptValue::Number(n);
    assert_eq!(
        params.unwrap(),
        LevelParams {
            level: "zulu".into(),
            script: "script\\vfx_level_load_zulu.lua".into(),
            calls: vec![
                ScriptCall {
                    function: "change_fog".into(),
                    args: vec![number(400.0), number(700.0)]
                },
                ScriptCall {
                    function: "enable_fog".into(),
                    args: vec![]
                },
                ScriptCall {
                    function: "set_back_light".into(),
                    args: vec![
                        number(0.5),
                        ScriptValue::Text("x".into()),
                        ScriptValue::Bool(true),
                        ScriptValue::Nil
                    ]
                },
            ]
        }
    );
    assert_eq!(report.read, 3);
    assert_eq!(
        report.unread,
        [(14, "an argument that is not a literal".to_string())]
    );
    assert!(!report.branching);
}

#[test]
fn a_level_without_a_script_is_a_report_line() {
    let pak = pak(&[("script\\vfx_level_load_zulu.lua", &script())]);
    let (params, report) = read_level_params(&ScriptInputs {
        scripts: &pak,
        level: "yankee",
    })
    .unwrap();
    assert!(params.is_none() && report.script.is_none());
    assert!(
        report.lines()[0]
            .1
            .contains("no vfx_level_load script for level yankee")
    );
}

#[test]
fn a_script_with_control_flow_is_not_read() {
    let pak = pak(&[("script\\vfx_level_load_zulu.lua", &branching_script())]);
    let (params, report) = read_level_params(&ScriptInputs {
        scripts: &pak,
        level: "zulu",
    })
    .unwrap();
    assert!(params.is_none() && report.branching && report.read == 0);
    assert!(report.lines()[0].1.contains("control flow"));
}

#[test]
fn a_script_that_is_not_a_chunk_is_refused_by_name() {
    let pak = pak(&[("script\\vfx_level_load_zulu.lua", b"print('hi')")]);
    let e = read_level_params(&ScriptInputs {
        scripts: &pak,
        level: "zulu",
    })
    .unwrap_err();
    assert!(e.to_string().contains("vfx_level_load_zulu.lua"), "{e}");
}

#[test]
fn the_reader_puts_the_parameters_in_the_world_as_a_resource() {
    let scripts = pak(&[("script\\vfx_level_load_zulu.lua", &script())]);
    let mut reader = ShredReader::new(vec![], SourceFrame::WORLD).with_scripts(ScriptInputs {
        scripts: &scripts,
        level: "zulu",
    });
    let (world, lines) = reader.read().unwrap();
    convert::check_emits(&world, reader.emits(), "read").unwrap();
    assert!(world.verify_resources().is_empty());
    let params = world.resource::<LevelParams>().unwrap();
    assert_eq!(params.calls_to("change_fog").count(), 1);
    assert!(
        lines
            .iter()
            .any(|(k, v)| k == "shred level params" && v.contains("3 calls read as data, 1 not read"))
    );
    assert_eq!(reader.params.read, 3);
}
