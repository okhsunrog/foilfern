//! Physical board viewing, corresponding to upstream's SceneBuilder3D/OpenGlView.
//! Core supplies canonical geometry; earcutr triangulates material regions and
//! Bevy handles GPU rendering. No CAD or GPU state enters core.
mod camera;
mod error;
mod mesh;
pub mod renderer;

pub use camera::OrbitCamera;
pub use error::{Error, Result};
pub use mesh::{BoardMesh, BoardSnapshot, MeshPart, SurfaceRole};
