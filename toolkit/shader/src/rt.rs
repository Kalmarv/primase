//! The ray tracing pipeline's operations. WGSL has no ray tracing pipeline and naga's has no `ReportHit`,
//! `IgnoreHit`, `CallShader` or hit-group indices, so a ray tracing program (see [`crate::stage_functions`]) does
//! what the pipeline does for it by calling functions of its own module that have no body: the call is the
//! operation, the arguments its operands, and a consumer that traces rays reads the calls. A stub's body is empty (a
//! function that returns a value returns `false`), so nothing that runs the module as written runs the pipeline.
//!
//! | name | arguments | result |
//! |------|-----------|--------|
//! | [`TRACE_RAY`] | acceleration structure, ray flags, instance mask, hit group offset, hit group stride, miss index, origin (`vec3<f32>`), t min, direction (`vec3<f32>`), t max, pointer to the payload | |
//! | [`REPORT_HIT`] | t (`f32`), hit kind (`u32`), pointer to the attributes | whether the hit was accepted (`bool`) |
//! | [`CALL_SHADER`] | shader index (`u32`), pointer to the parameter | |
//! | [`IGNORE_HIT`] | | (the program ends) |
//! | [`ACCEPT_HIT_AND_END_SEARCH`] | | (the program ends) |

/// `TraceRay`: shoot a ray into an acceleration structure; the payload is read and written by the programs it runs.
pub const TRACE_RAY: &str = "trace_ray";
/// `ReportHit`: an intersection program reports a hit at `t`.
pub const REPORT_HIT: &str = "report_hit";
/// `CallShader`: run a callable program.
pub const CALL_SHADER: &str = "call_shader";
/// `IgnoreHit`: an any-hit program rejects the candidate hit and ends.
pub const IGNORE_HIT: &str = "ignore_hit";
/// `AcceptHitAndEndSearch`: an any-hit program accepts the hit, stops the search and ends.
pub const ACCEPT_HIT_AND_END_SEARCH: &str = "accept_hit_and_end_search";

/// Every operation, by name.
pub const OPERATIONS: [&str; 5] = [
    TRACE_RAY,
    REPORT_HIT,
    CALL_SHADER,
    IGNORE_HIT,
    ACCEPT_HIT_AND_END_SEARCH,
];
