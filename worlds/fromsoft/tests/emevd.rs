//! The event-script reader on synthetic files built here in the stored layout (header, events, instructions, layers,
//! arguments padded to 4 and the block to 16, parameters, linked files, strings): a script reads back as written, and
//! every structural fault is an error.
use primase_fromsoft::emevd::{Event, Instruction, Parameter, Script};

/// One synthetic script in Sekiro's layout.
fn build(events: &[Event], layers: &[u32], linked: &[i64], strings: &[u8]) -> Vec<u8> {
    let n_ins: usize = events.iter().map(|e| e.instructions.len()).sum();
    let n_par: usize = events.iter().map(|e| e.parameters.len()).sum();
    let events_at = 0x90;
    let instructions_at = events_at + 0x30 * events.len();
    let layers_at = instructions_at + 0x20 * n_ins;
    let args_at = layers_at + 0x20 * layers.len();
    let mut args = Vec::new();
    let mut arg_offsets = Vec::new();
    for e in events {
        for i in &e.instructions {
            arg_offsets.push(if i.args.is_empty() {
                -1i64
            } else {
                args.len() as i64
            });
            args.extend_from_slice(&i.args);
            args.resize(args.len().next_multiple_of(4), 0);
        }
    }
    args.resize(args.len().next_multiple_of(16), 0);
    let parameters_at = args_at + args.len();
    let linked_at = parameters_at + 0x20 * n_par;
    let strings_at = linked_at + 8 * linked.len();
    let size = strings_at + strings.len();

    let mut d = b"EVD\0\x00\xff\x01\xff".to_vec();
    d.extend(0xCDu32.to_le_bytes());
    d.extend((size as u32).to_le_bytes());
    for v in [
        events.len(),
        events_at,
        n_ins,
        instructions_at,
        0,
        layers_at,
        layers.len(),
        layers_at,
        n_par,
        parameters_at,
        linked.len(),
        linked_at,
        args.len(),
        args_at,
        strings.len(),
        strings_at,
    ] {
        d.extend((v as u64).to_le_bytes());
    }
    let (mut ins_index, mut par_index) = (0, 0);
    for e in events {
        d.extend(e.id.to_le_bytes());
        d.extend((e.instructions.len() as u64).to_le_bytes());
        d.extend((ins_index * 0x20u64).to_le_bytes());
        d.extend((e.parameters.len() as u64).to_le_bytes());
        d.extend((par_index * 0x20u64).to_le_bytes());
        d.extend(e.rest.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        ins_index += e.instructions.len() as u64;
        par_index += e.parameters.len() as u64;
    }
    let mut k = 0;
    for e in events {
        for i in &e.instructions {
            d.extend(i.bank.to_le_bytes());
            d.extend(i.id.to_le_bytes());
            d.extend((i.args.len() as u64).to_le_bytes());
            d.extend(arg_offsets[k].to_le_bytes());
            let layer = i.layer.map_or(-1i64, |l| {
                let at = layers
                    .iter()
                    .position(|&x| x == l)
                    .expect("the layer is in the table");
                0x20 * at as i64
            });
            d.extend(layer.to_le_bytes());
            k += 1;
        }
    }
    for &l in layers {
        d.extend(2u32.to_le_bytes());
        d.extend(l.to_le_bytes());
        d.extend(0i64.to_le_bytes());
        d.extend((-1i64).to_le_bytes());
        d.extend(1i64.to_le_bytes());
    }
    d.extend(&args);
    for e in events {
        for p in &e.parameters {
            for v in [p.instruction, p.target, p.source] {
                d.extend((v as u64).to_le_bytes());
            }
            d.extend(p.bytes.to_le_bytes());
            d.extend(p.unk.to_le_bytes());
        }
    }
    for l in linked {
        d.extend(l.to_le_bytes());
    }
    d.extend(strings);
    assert_eq!(d.len(), size);
    d
}

fn args(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Event 50 initialising common function 20004110 for entity group 1117204, disabled while flag 8302 is off;
/// and a common event whose instruction takes its entity from the caller's arguments.
fn sample() -> Vec<Event> {
    vec![
        Event {
            id: 50,
            rest: 0,
            instructions: vec![Instruction {
                bank: 2000,
                id: 6,
                args: args(&[20004110, 1117204, 8302, 0, 0, 0, 0x01010100]),
                layer: None,
            }],
            parameters: vec![],
        },
        Event {
            id: 20004110,
            rest: 0,
            instructions: vec![
                Instruction {
                    bank: 2012,
                    id: 1,
                    args: args(&[0, 0]),
                    layer: Some(7),
                },
                Instruction {
                    bank: 1014,
                    id: 0,
                    args: vec![],
                    layer: None,
                },
                Instruction {
                    bank: 3,
                    id: 0,
                    // 10 bytes: two bytes of alignment follow
                    args: vec![0xff, 1, 0, 0, 0, 0, 0, 0, 0x6d, 0x20],
                    layer: None,
                },
            ],
            parameters: vec![Parameter {
                instruction: 0,
                target: 0,
                source: 0,
                bytes: 4,
                unk: 0,
            }],
        },
    ]
}

#[test]
fn a_script_reads_back_as_written() {
    let events = sample();
    let data = build(&events, &[7], &[0x40], b"N:\0\0");
    let s = Script::read(&data).unwrap();
    assert_eq!(s.events, events);
    assert_eq!(s.linked_files, [0x40]);
    assert_eq!(s.strings, b"N:\0\0");
    assert_eq!(
        s.events[0].instructions[0].args_i32().unwrap(),
        [20004110, 1117204, 8302, 0, 0, 0, 0x01010100]
    );
    assert_eq!(Script::read(&build(&[], &[], &[], &[])).unwrap().events, []);
}

#[test]
fn every_truncation_is_an_error() {
    let data = build(&sample(), &[7], &[], &[]);
    for end in 0..data.len() {
        assert!(Script::read(&data[..end]).is_err(), "{end}");
    }
}

#[test]
fn structural_faults_are_errors() {
    let good = build(&sample(), &[7], &[], &[]);
    let fault = |at: usize, bytes: &[u8]| {
        let mut d = good.clone();
        d[at..at + bytes.len()].copy_from_slice(bytes);
        Script::read(&d)
    };
    assert!(fault(0, b"EVE").is_err(), "magic");
    assert!(fault(5, &[0]).is_err(), "32-bit layout");
    assert!(fault(8, &0xCCu32.to_le_bytes()).is_err(), "older version");
    assert!(
        fault(12, &((good.len() + 1) as u32).to_le_bytes()).is_err(),
        "file size"
    );
    // the unknown table is not empty
    assert!(fault(0x10 + 4 * 8, &1u64.to_le_bytes()).is_err(), "unknown table");
    let events_at = 0x90;
    // rest behaviour outside 0..=2, and nonzero padding after it
    assert!(fault(events_at + 40, &3u32.to_le_bytes()).is_err(), "rest");
    assert!(
        fault(events_at + 44, &1u32.to_le_bytes()).is_err(),
        "event padding"
    );
    // the second event's instructions claimed from the start of the table again
    assert!(
        fault(events_at + 0x30 + 16, &0u64.to_le_bytes()).is_err(),
        "instructions owned twice"
    );
    let instructions_at = events_at + 2 * 0x30;
    // a layer offset that is not a layer record
    assert!(
        fault(instructions_at + 0x20 + 24, &8i64.to_le_bytes()).is_err(),
        "layer offset"
    );
    // arguments past the argument block
    assert!(
        fault(instructions_at + 0x20 + 8, &64u64.to_le_bytes()).is_err(),
        "argument length"
    );
    let layers_at = instructions_at + 4 * 0x20;
    assert!(fault(layers_at, &3u32.to_le_bytes()).is_err(), "layer record");
    let parameters_at = good.len() - 0x20;
    // a substitution past its instruction's arguments, and one of zero bytes
    assert!(
        fault(parameters_at + 8, &6u64.to_le_bytes()).is_err(),
        "parameter target"
    );
    assert!(
        fault(parameters_at + 24, &0u32.to_le_bytes()).is_err(),
        "parameter of no bytes"
    );
    assert!(
        fault(parameters_at, &3u64.to_le_bytes()).is_err(),
        "parameter instruction"
    );
    // a nonzero byte in the argument block's padding
    let args_at = layers_at + 0x20;
    // a nonzero byte in the alignment after the 10-byte arguments (28 + 8 + 10 = 46)
    assert!(fault(args_at + 46, &[1]).is_err(), "unaccounted byte");
    assert!(Script::read(&good).is_ok());
    // huge counts in an event are errors before anything is sized by them
    assert!(
        fault(events_at + 8, &(i64::MAX as u64).to_le_bytes()).is_err(),
        "huge instruction count"
    );
    assert!(
        fault(events_at + 24, &(i64::MAX as u64).to_le_bytes()).is_err(),
        "huge parameter count"
    );
    assert!(
        fault(events_at + 0x30 + 8, &4u64.to_le_bytes()).is_err(),
        "one more instruction than the table holds"
    );
    // an instruction no event owns
    let mut d = good.clone();
    d[0x10 + 2 * 8..0x10 + 3 * 8].copy_from_slice(&5u64.to_le_bytes());
    assert!(Script::read(&d).is_err(), "instruction count");
}
