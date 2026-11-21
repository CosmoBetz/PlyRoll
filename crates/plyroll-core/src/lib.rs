//! PlyRoll core: geometry, DXF import, ply/material model, fibre-orientation
//! rules, layout verification and export. The nesting solver lives in
//! `plyroll-nest` so this crate stays free of solver dependencies.

pub mod export;
pub mod geom;
pub mod import;
pub mod job;
pub mod model;
pub mod orient;
pub mod project;
pub mod report;
pub mod verify;
pub mod view;
