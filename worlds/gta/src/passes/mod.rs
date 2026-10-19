//! The GTA passes and the tables only they use: lowerings to core's meanings, the export's selections and the shader
//! tables. No I/O: each takes the world and its settings.
pub mod interiors;
pub mod lights;
pub mod lower;
pub mod pose;
pub mod rect;
pub mod roles;
pub mod select;
pub mod shader_class;
pub mod shader_names;
pub mod surfaces;
pub mod terrain;
pub mod winding;
