//! How many arguments each `dx.op` the lifter carries is called with, the opcode among them. The call's callee fixes the
//! count (`dx.op.unary.f32` takes the opcode and one value), and the opcode picks the operation: a program whose opcode
//! and callee disagree is not a DXIL program, and the lifter reads an operation's arguments by position, so the count is
//! checked once, before the operation is dispatched.
//!
//! The counts are those the 69,826 modules of the oracle corpora and the committed fixtures call the operations with
//! (every opcode was called with one count only). The operations none of them calls (the integer multiplies, divide and
//! carry operations 41 to 45, `Fma`, `LoadOutputControlPoint`, some of the mesh and ray query operations, `IsNormal` and
//! the hyperbolic functions) have the parameter list the DXIL specification gives them; no program checks those.

/// The number of arguments (the opcode included) of the `dx.op` with this opcode, if the lifter carries it.
pub(crate) fn arguments(code: u64) -> Option<usize> {
    Some(match code {
        90 | 96 | 100 | 107..=108 | 110..=112 | 141..=144 | 153..=156 | 161 | 170 | 213 => 1,
        6..=34
        | 80
        | 82..=86
        | 93..=95
        | 97..=99
        | 105
        | 113..=116
        | 118
        | 130..=131
        | 135..=136
        | 145..=150
        | 160
        | 178
        | 180..=182
        | 184..=185
        | 190..=192
        | 195
        | 198..=204
        | 207..=210
        | 214..=215 => 2,
        35..=45
        | 59
        | 70
        | 72
        | 75
        | 117
        | 120
        | 122..=123
        | 151..=152
        | 159
        | 168
        | 183
        | 193..=194
        | 196..=197
        | 205..=206
        | 211..=212
        | 216 => 3,
        46..=49 | 68 | 104 | 119 | 121 | 158 | 186..=189 | 217..=218 => 4,
        4..=5 | 54 | 57 | 103 | 106 | 169 | 173 => 5,
        139 | 171..=172 => 6,
        55 | 78..=79 => 7,
        56 | 66 | 69 => 9,
        67 | 73 | 140 => 10,
        60 | 62 | 65 | 74 => 11,
        61 | 64 => 12,
        179 => 13,
        157 => 16,
        63 => 17,
        _ => return None,
    })
}
