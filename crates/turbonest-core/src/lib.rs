//! TurboNest core: geometry, DXF import, ply/material model, fibre-orientation
//! rules, layout verification and export. The nesting solver lives in
//! `turbonest-nest` so this crate stays free of solver dependencies.

pub mod export;
pub mod geom;
pub mod import;
pub mod job;
pub mod model;
pub mod orient;
pub mod verify;
