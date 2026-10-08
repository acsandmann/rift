mod backdrop;
mod layer;
mod private;
pub use private::G_CONNECTION;
mod surface;
mod transaction;
mod window;
pub use backdrop::backdrop_blur;
pub use layer::{LayerTransaction, release_layer_tree, render_layer_to_cgs_window, with_disabled_actions};
pub use surface::WindowSurface;
pub use transaction::WindowTransaction;
pub use window::{CgsWindow, CgsWindowError};
fn cg_ok(error: objc2_core_graphics::CGError) -> Result<(), objc2_core_graphics::CGError> {
    if error == objc2_core_graphics::CGError::Success {
        Ok(())
    } else {
        Err(error)
    }
}

pub fn main_connection() -> i32 { unsafe { private::SLSMainConnectionID() } }
