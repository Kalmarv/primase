//! Shaun White Skateboarding (PC): the records its version-26 forges store, read from the objects of an entry
//! ([`crate::objects`]): scene entities and their components ([`scene`]), collision shapes ([`collision`]), spawn
//! markers ([`markers`]), grind guidance ([`guidance`]), lights and the sky's settings ([`light`]), materials
//! ([`material`], [`shader_material`]), and the SDB2 shader databases shipped beside the forges ([`shaders`]).

pub mod collision;
pub mod guidance;
pub mod light;
pub mod markers;
pub mod material;
pub mod scene;
pub mod shader_material;
pub mod shaders;
