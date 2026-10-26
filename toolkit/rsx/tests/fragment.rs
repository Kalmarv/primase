use primase_rsx::fragment::{Control, Opcode, Register, SourceKind, decode, flow::analyze};

fn encode(words: &[u32]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|w| w.rotate_left(16).to_be_bytes())
        .collect()
}
// Numeric bit positions are laid out by hand. No production encoder is used.
const XYZW: u32 = (1 << 11) | (2 << 13) | (3 << 15);
fn instruction(op: u32, reg: u32, end: bool) -> [u32; 4] {
    [
        u32::from(end) | (reg << 1) | (15 << 9) | ((op & 63) << 24),
        XYZW | (7 << 18),
        XYZW | ((op >> 6) << 31),
        XYZW,
    ]
}
fn sample(reg: u32, unit: u32, end: bool) -> [u32; 4] {
    let mut w = instruction(23, reg, end);
    w[0] |= (unit << 17) | (4 << 13);
    w[1] |= 1;
    w
}
fn r(index: u8) -> Register {
    Register { index, half: false }
}
fn mask(bytes: &[u8], register: Register) -> [u16; 4] {
    analyze(&decode(bytes).expect("decode"), &[register])
        .expect("analysis")
        .outputs[0]
        .components
        .map(|d| d.textures)
}

#[test]
fn word_rotation_fields_sources_and_modifiers_are_retained() {
    let mut w = instruction(4, 9, true);
    w[0] |= (1 << 7) | (1 << 8) | (1 << 21) | (2 << 22) | (1 << 31);
    w[1] = 2
        | XYZW
        | (1 << 17)
        | (5 << 18)
        | (2 << 21)
        | (1 << 23)
        | (3 << 25)
        | (1 << 29)
        | (1 << 30)
        | (1 << 31);
    w[2] = (3 << 2) | (1 << 8) | XYZW | (1 << 18) | (1 << 19) | (2 << 22) | (3 << 25) | (5 << 28);
    w[3] = (4 << 2) | XYZW | (1 << 17) | (1 << 18);
    let mut bytes = encode(&w);
    bytes.extend(encode(&[0x3f800000, 0x80000000, 0x7fc01234, 0x40400000]));
    let p = decode(&bytes).unwrap();
    let i = &p.instructions[0];
    assert_eq!(
        (i.opcode, i.offset, i.size, i.destination),
        (Opcode::MAD, 0, 32, Register { index: 9, half: true })
    );
    assert_eq!(
        (
            i.write_mask,
            i.update_condition,
            i.saturate,
            i.precision,
            i.expand_texture
        ),
        (15, true, true, 2, true)
    );
    assert_eq!(
        (
            i.predicate.test,
            i.predicate.register,
            i.condition_destination,
            i.predicate.swizzle
        ),
        (5, 1, 1, [2, 1, 3, 0])
    );
    assert_eq!((i.source_precision, i.scale), ([1, 2, 3], 5));
    assert_eq!(i.sources[0].kind, SourceKind::Constant);
    assert!(i.sources[0].absolute && i.sources[0].negate);
    assert_eq!(
        i.sources[1].kind,
        SourceKind::Register(Register { index: 3, half: true })
    );
    assert!(i.sources[1].absolute && i.sources[2].absolute && i.sources[2].negate);
    assert_eq!(i.constant, Some([0x3f800000, 0x80000000, 0x7fc01234, 0x40400000]));
    let text = p.disassemble();
    assert!(text.contains("MAD h9.xyzw") && text.contains("pred=5 cc1.zywx") && text.contains("sat=true"));
}

#[test]
fn texture_unit_is_separate_from_coordinate_input_and_has_a_def_use_path() {
    let a = sample(1, 3, false);
    let mut b = instruction(1, 0, true);
    b[1] |= 1 << 2;
    let p = decode(&encode(&[a, b].concat())).unwrap();
    let flow = analyze(&p, &[r(0)]).unwrap();
    assert_eq!(flow.samples[0].unit, 3);
    assert_eq!(flow.samples[0].coordinates.inputs, 1 << 4);
    assert_eq!(flow.outputs[0].components.map(|d| d.textures), [1 << 3; 4]);
    assert_eq!(flow.consumers[3], [Opcode::MOV]);
    assert!(p.disassemble().contains("tex3"));
}

#[test]
fn unconditional_overwrites_remove_dependencies_and_half_writes_alias_full_registers() {
    let a = sample(0, 3, false);
    let mut b = instruction(1, 0, true);
    b[1] |= 2;
    let mut bytes = encode(&[a, b].concat());
    bytes.extend(encode(&[0; 4]));
    assert_eq!(mask(&bytes, r(0)), [0; 4]);
    b[0] |= 1 << 7; // H0 overlaps the low half (two full components) of R0.
    let mut bytes = encode(&[a, b].concat());
    bytes.extend(encode(&[0; 4]));
    assert_eq!(mask(&bytes, r(0)), [0, 0, 1 << 3, 1 << 3]);
}

#[test]
fn conditional_writes_merge_and_never_executed_samples_are_not_influences() {
    let a = sample(0, 1, false);
    let mut b = sample(0, 2, true);
    b[1] = (b[1] & !(7 << 18)) | (2 << 18); // EQ, unlike unconditional (7)
    assert_eq!(mask(&encode(&[a, b].concat()), r(0)), [(1 << 1) | (1 << 2); 4]);
    b[1] &= !(7 << 18);
    assert_eq!(mask(&encode(&[a, b].concat()), r(0)), [1 << 1; 4]);
}

#[test]
fn if_else_paths_and_texture_conditioned_kill_are_reported() {
    let mut branch = instruction(66, 0, false);
    branch[1] = 6 << 18;
    branch[2] = (1 << 31) | 12;
    branch[3] = 16;
    let code = [
        branch,
        sample(0, 1, false),
        instruction(0, 0, false),
        sample(0, 2, false),
        instruction(0, 0, true),
    ]
    .concat();
    let p = decode(&encode(&code)).unwrap();
    assert_eq!(
        p.instructions[0].control,
        Some(Control::If {
            else_offset: 48,
            end_offset: 64
        })
    );
    assert_eq!(mask(&encode(&code), r(0)), [(1 << 1) | (1 << 2); 4]);
    let a = sample(0, 3, false);
    let mut cc = instruction(1, 63, false);
    cc[0] |= (1 << 30) | (1 << 8);
    let mut kill = instruction(18, 0, true);
    kill[1] = (kill[1] & !(7 << 18)) | (2 << 18);
    let flow = analyze(&decode(&encode(&[a, cc, kill].concat())).unwrap(), &[r(0)]).unwrap();
    assert_eq!(flow.kill.textures, 1 << 3);
}

#[test]
fn malformed_programs_and_unproven_flow_cases_refuse_explicitly() {
    let good = encode(&instruction(1, 0, true));
    for len in 0..good.len() {
        assert!(decode(&good[..len]).is_err());
    }
    let mut bad = instruction(1, 0, false);
    assert!(decode(&encode(&bad)).unwrap_err().detail.contains("no END"));
    bad[0] |= 1;
    bad[1] |= 3;
    assert!(
        decode(&encode(&bad))
            .unwrap_err()
            .detail
            .contains("source register")
    );
    bad = instruction(1, 0, true);
    bad[1] |= 2;
    assert!(decode(&encode(&bad)).is_err());
    let mut bytes = good;
    bytes.extend([0; 16]);
    assert!(decode(&bytes).unwrap_err().detail.contains("after END"));
    assert!(
        decode(&encode(&instruction(48, 0, true)))
            .unwrap_err()
            .detail
            .contains("unsupported")
    );
    let p = decode(&encode(&instruction(65, 0, true))).unwrap();
    assert!(analyze(&p, &[r(0)]).unwrap_err().detail.contains("calls"));
    let mut branch = instruction(66, 0, false);
    branch[2] |= 5;
    branch[3] = 8;
    assert!(
        decode(&encode(&[branch, instruction(0, 0, true)].concat()))
            .unwrap_err()
            .detail
            .contains("IF boundaries")
    );
}

#[test]
fn predicates_use_all_three_outcome_bits_including_never_equal_and_always() {
    for (test, name) in ["FL", "LT", "EQ", "LE", "GT", "NE", "GE", "TR"]
        .into_iter()
        .enumerate()
    {
        let mut w = instruction(1, 0, true);
        w[1] = (w[1] & !(7 << 18)) | ((test as u32) << 18);
        assert_eq!(
            decode(&encode(&w)).unwrap().instructions[0].predicate.condition(),
            name
        );
    }
}

#[test]
fn empty_then_blocks_skip_else_and_conditional_return_keeps_control_dependence() {
    let mut branch = instruction(66, 0, false);
    branch[2] = (1 << 31) | 4;
    branch[3] = 8;
    let code = [branch, sample(0, 2, false), instruction(0, 0, true)].concat();
    assert_eq!(mask(&encode(&code), r(0)), [0; 4]); // true condition, empty then block
    let a = sample(1, 2, false);
    let mut cc = instruction(1, 63, false);
    cc[0] |= (1 << 30) | (1 << 8);
    cc[1] |= 1 << 2;
    let mut ret = instruction(69, 0, false);
    ret[1] = (ret[1] & !(7 << 18)) | (2 << 18);
    let mut zero = instruction(1, 0, true);
    zero[1] |= 2;
    let mut bytes = encode(&[a, cc, ret, zero].concat());
    bytes.extend(encode(&[0; 4]));
    assert_eq!(mask(&bytes, r(0)), [1 << 2; 4]);
}

#[test]
fn control_only_texture_reads_do_not_invent_writes_to_untouched_outputs() {
    let a = sample(1, 3, false);
    let mut cc = instruction(1, 63, false);
    cc[0] |= (1 << 30) | (1 << 8);
    cc[1] |= 1 << 2;
    let mut branch = instruction(66, 0, false);
    branch[1] = 2 << 18;
    branch[2] = (1 << 31) | 12;
    branch[3] = 12;
    let code = [a, cc, branch, instruction(0, 0, true)].concat();
    assert_eq!(mask(&encode(&code), r(0)), [0; 4]);
}

#[test]
fn separate_lod_bias_and_gradient_sources_are_not_lost_when_oracle_text_omits_them() {
    for op in [47, 49] {
        // TXL/TXB use source0 coordinates, source1.x LOD/bias
        let mut w = instruction(op, 0, true);
        w[1] |= 1;
        w[2] = 2 | XYZW;
        let mut bytes = encode(&w);
        bytes.extend(encode(&[0x40000000, 0, 0, 0]));
        let p = decode(&bytes).unwrap();
        assert_eq!(p.instructions[0].sources.len(), 2);
        assert_eq!(p.instructions[0].sources[1].kind, SourceKind::Constant);
        assert_eq!(p.instructions[0].constant, Some([0x40000000, 0, 0, 0]));
        assert_eq!(p.instructions[0].size, 32);
        w[0] &= !1;
        let mut followed = encode(&w);
        followed.extend(encode(&[0x40000000, 0, 0, 0]));
        followed.extend(encode(&instruction(1, 0, true)));
        let p = decode(&followed).unwrap();
        assert_eq!(p.instructions.len(), 2);
        assert_eq!(p.instructions[1].offset, 32);
        assert_eq!(p.instructions[1].opcode, Opcode::MOV);
    }
    let mut grad = instruction(25, 0, true);
    grad[3] = 2 | XYZW;
    let mut bytes = encode(&grad);
    bytes.extend(encode(&[1, 2, 3, 4]));
    assert_eq!(decode(&bytes).unwrap().instructions[0].sources.len(), 3);
    let a = sample(1, 3, false);
    let mut lod = instruction(47, 0, true);
    lod[0] |= 4 << 17;
    lod[1] |= 1;
    lod[2] = 1 << 2; // source1 R1.x controls LOD
    let p = decode(&encode(&[a, lod].concat())).unwrap();
    let flow = analyze(&p, &[r(0)]).unwrap();
    assert_eq!(flow.samples[1].coordinates.textures, 0);
    assert_eq!(flow.samples[1].arguments.textures, 1 << 3);
    assert_eq!(
        flow.outputs[0].components.map(|d| d.textures),
        [(1 << 3) | (1 << 4); 4]
    );
}
