//! How many operands each instruction the lifter carries has. The token format fixes the count per opcode (a destination
//! or two, then the sources, a resource and a sampler among them), so a program whose instruction has another count is
//! not a program of this format; the lifter checks it once, before it reads an operand by position.

/// The operand count of the instruction `name` (as [`crate::decode::opcode_name`] spells it), for the instructions the
/// lifter carries; `None` for any other.
pub(crate) fn operands(name: &str) -> Option<usize> {
    Some(match name {
        "nop" | "else" | "endif" | "loop" | "endloop" | "break" | "continue" | "default" | "endswitch"
        | "ret" | "sync" | "emit" | "cut" | "emitthencut" => 0,
        "if" | "breakc" | "continuec" | "switch" | "case" | "retc" | "discard" | "emit_stream"
        | "cut_stream" | "emitthencut_stream" => 1,
        "mov" | "rcp" | "rsq" | "sqrt" | "exp" | "log" | "frc" | "round_ne" | "round_ni" | "round_pi"
        | "round_z" | "deriv_rtx" | "deriv_rty" | "deriv_rtx_coarse" | "deriv_rty_coarse"
        | "deriv_rtx_fine" | "deriv_rty_fine" | "not" | "ineg" | "itof" | "utof" | "ftoi" | "ftou"
        | "countbits" | "bfrev" | "firstbit_hi" | "firstbit_shi" | "firstbit_lo" | "f32tof16"
        | "f16tof32" | "sample_info" => 2,
        "add" | "mul" | "div" | "min" | "max" | "dp2" | "dp3" | "dp4" | "sincos" | "eq" | "ne" | "lt"
        | "ge" | "ieq" | "ine" | "ilt" | "ige" | "ult" | "uge" | "and" | "or" | "xor" | "iadd" | "imin"
        | "imax" | "umin" | "umax" | "ishl" | "ushr" | "ishr" | "ld" | "ld_raw" | "ld_uav_typed"
        | "store_uav_typed" | "store_raw" | "resinfo" => 3,
        "movc" | "mad" | "imad" | "umad" | "imul" | "umul" | "udiv" | "uaddc" | "usubb" | "ubfe" | "ibfe"
        | "ld_ms" | "ld_structured" | "store_structured" | "sample" | "gather4" => 4,
        "swapc" | "bfi" | "sample_b" | "sample_l" | "sample_c" | "sample_c_lz" | "gather4_c" => 5,
        "sample_d" => 6,
        // append and consume buffer counters: [destination, view]
        "imm_atomic_alloc" | "imm_atomic_consume" => 2,
        // [view, address, source], and a compare-and-store's [view, address, compare, new value]
        "atomic_cmp_store" => 4,
        n if n.starts_with("atomic_") => 3,
        // [old value, view, address, source], and a compare-and-exchange's [old value, view, address, compare, new]
        "imm_atomic_cmp_exch" => 5,
        n if n.starts_with("imm_atomic_") => 4,
        _ => return None,
    })
}
