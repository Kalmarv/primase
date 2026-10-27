#![allow(clippy::unwrap_used)]
//! Programs a malformed container could hold, made from the committed fixtures by changing the words the case is about:
//! each is refused by name, never a panic or an allocation the declaration asked for. The oracle is the token format's
//! own rules (D3D11's limits, an opcode's operand count); nothing here is a second reader.
use primase_dxbc::decode::Declared;
use primase_dxbc::{decode, lift};
use dxcontainer::chunks;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn code(container: &[u8]) -> Vec<u32> {
    let parts = chunks(container).unwrap();
    let part = parts
        .iter()
        .find(|c| &c.kind == b"SHEX" || &c.kind == b"SHDR")
        .unwrap();
    part.data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect()
}

/// The container with its program replaced by `words` (its length token set), every other part as it was.
fn rebuild(container: &[u8], mut words: Vec<u32>) -> Vec<u8> {
    words[1] = words.len() as u32;
    let parts = chunks(container).unwrap();
    let header = 32 + 4 * parts.len();
    let (mut offsets, mut body) = (Vec::new(), Vec::new());
    for part in &parts {
        offsets.push((header + body.len()) as u32);
        let data: Vec<u8> = if &part.kind == b"SHEX" || &part.kind == b"SHDR" {
            words.iter().flat_map(|w| w.to_le_bytes()).collect()
        } else {
            part.data.to_vec()
        };
        body.extend_from_slice(&part.kind);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&data);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    let mut out = b"DXBC".to_vec();
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&((header + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    for offset in offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// The first word of the first declaration `kind` picks.
fn declaration(words: &[u32], kind: impl Fn(&Declared, u16) -> bool) -> usize {
    decode(words)
        .unwrap()
        .declarations
        .iter()
        .find(|d| kind(&d.kind, d.opcode))
        .expect("the fixture has the declaration")
        .word
}

fn instruction(words: &[u32], name: &str) -> usize {
    decode(words)
        .unwrap()
        .instructions
        .iter()
        .find(|i| i.name() == name)
        .expect("the fixture has the instruction")
        .word
}

/// A declaration's length in words, from its opcode token.
fn length(words: &[u32], at: usize) -> usize {
    ((words[at] >> 24) & 127) as usize
}

fn refusal(container: &[u8]) -> String {
    match lift(container) {
        Ok(_) => panic!("the program was lifted"),
        Err(e) => e.detail,
    }
}

fn edited(name: &str, edit: impl FnOnce(&mut Vec<u32>)) -> Vec<u8> {
    let container = fixture(name);
    let mut words = code(&container);
    edit(&mut words);
    rebuild(&container, words)
}

#[test]
fn the_unchanged_fixtures_still_lift() {
    for name in [
        "image_atomics.dxbc",
        "compute_basic.dxbc",
        "tessellation.hs.dxbc",
        "tessellation.vs.dxbc",
    ] {
        let container = fixture(name);
        lift(&rebuild(&container, code(&container))).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn dcl_temps_past_the_limit_is_refused() {
    let bytes = edited("image_atomics.dxbc", |w| {
        let at = declaration(w, |k, _| matches!(k, Declared::Temps(_)));
        w[at + 1] = u32::MAX;
    });
    assert!(refusal(&bytes).contains("dcl_temps 4294967295"));
}

#[test]
fn dcl_indexable_temp_past_the_limit_is_refused() {
    // dcl_temps (two words) rewritten as dcl_indexable_temp x0[2^30], four components (four words)
    let bytes = edited("image_atomics.dxbc", |w| {
        let at = declaration(w, |k, _| matches!(k, Declared::Temps(_)));
        w.splice(at..at + 2, [105 | (4 << 24), 0, 1 << 30, 4]);
    });
    assert!(refusal(&bytes).contains("dcl_indexable_temp x0[1073741824]"));
}

#[test]
fn a_constant_buffer_past_the_limit_is_refused() {
    let bytes = edited("tessellation.vs.dxbc", |w| {
        let at = declaration(w, |k, _| matches!(k, Declared::ConstantBuffer { .. }));
        let last = at + length(w, at) - 1;
        w[last] = i32::MAX as u32;
    });
    assert!(refusal(&bytes).contains("rows"));
}

#[test]
fn thread_group_memory_past_the_limit_is_refused() {
    for (kind, name) in [(159u16, "dcl_tgsm_raw"), (160, "dcl_tgsm_structured")] {
        let bytes = edited("compute_basic.dxbc", |w| {
            let at = declaration(w, |_, opcode| opcode == kind);
            let last = at + length(w, at) - 1;
            w[last] = i32::MAX as u32;
        });
        assert!(refusal(&bytes).contains(name), "{name}");
    }
}

#[test]
fn dcl_index_range_past_the_limit_or_of_another_register_is_refused() {
    let bytes = edited("tessellation.hs.dxbc", |w| {
        let at = declaration(w, |_, opcode| opcode == 91);
        let last = at + length(w, at) - 1;
        w[last] = u32::MAX;
    });
    assert!(refusal(&bytes).contains("dcl_index_range of 4294967295 registers"));
    // the operand names a temporary: only inputs and outputs are lifted
    let bytes = edited("tessellation.hs.dxbc", |w| {
        let at = declaration(w, |_, opcode| opcode == 91);
        w[at + 1] &= !(255 << 12);
    });
    assert!(refusal(&bytes).contains("dcl_index_range of operand register type 0"));
}

#[test]
fn a_declaration_with_words_after_its_operands_is_refused() {
    let bytes = edited("image_atomics.dxbc", |w| {
        let at = declaration(w, |k, _| matches!(k, Declared::Temps(_)));
        w.splice(at..at + 2, [104 | (3 << 24), 1, 0]);
    });
    assert!(refusal(&bytes).contains("1 words after the operands of dcl_temps"));
}

#[test]
fn a_phase_instance_count_of_two_words_is_refused() {
    let bytes = edited("tessellation.hs.dxbc", |w| {
        let at = declaration(w, |_, opcode| opcode == 153);
        w.splice(at..at + 2, [153 | (3 << 24), 2, 0]);
    });
    assert!(refusal(&bytes).contains("a phase's instance count of 2 words"));
}

#[test]
fn an_instruction_with_another_operand_count_is_refused() {
    // `mov` (two operands) with `mad`'s opcode, and `mul` (three) with `mov`'s
    let bytes = edited("tessellation.vs.dxbc", |w| {
        let at = instruction(w, "mov");
        w[at] = (w[at] & !0x7ff) | 50;
    });
    assert!(refusal(&bytes).contains("mad: 2 operands, where the instruction takes 4"));
    let bytes = edited("tessellation.vs.dxbc", |w| {
        let at = instruction(w, "mul");
        w[at] = (w[at] & !0x7ff) | 54;
    });
    assert!(refusal(&bytes).contains("mov: 3 operands, where the instruction takes 2"));
}

#[test]
fn an_opcode_the_lifter_does_not_carry_is_refused_with_its_number() {
    let bytes = edited("tessellation.vs.dxbc", |w| {
        let at = instruction(w, "mov");
        w[at] = (w[at] & !0x7ff) | 228;
    });
    assert!(refusal(&bytes).contains("opcode 228 is not lifted"));
}

#[test]
fn a_resource_operand_of_another_register_is_refused() {
    // the view of `store_uav_typed` made a temporary
    let bytes = edited("compute_basic.dxbc", |w| {
        let at = instruction(w, "store_uav_typed");
        w[at + 1] &= !(255 << 12);
    });
    assert!(refusal(&bytes).contains("operand 0 is Temp"));
}

#[test]
fn a_destination_of_no_lanes_is_refused() {
    let bytes = edited("tessellation.vs.dxbc", |w| {
        let at = instruction(w, "mov");
        w[at + 1] &= !(15 << 4);
    });
    assert!(refusal(&bytes).contains("write mask is empty"));
}
