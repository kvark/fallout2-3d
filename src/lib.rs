//! Fallout 2 exploration on `maps\arvillag.map`: original archives, hex steps,
//! INT procedures, and four corners drawn by blade-graphics.

pub mod art;
pub mod camera;
pub mod dat2;
pub mod direction;
pub mod frm;
pub mod hexgrid;
pub mod mapfile;
pub mod occlusion;
pub mod play;
pub mod render;
pub mod scene;
pub mod window;
pub mod script;

pub use camera::CornerCamera;
pub use dat2::GameFiles;
pub use direction::pick_direction;
pub use hexgrid::step;
pub use occlusion::point_hidden;
pub use play::{session_commands, AttackKind, Command, Game, Outcome, Strike};
pub use scene::build_scene;
