mod app;
mod bridge;
mod collection;
pub mod compositor;
mod control;
mod drawing;
mod help;
mod input;
mod layout;
mod menu;
mod navigation;
mod preview;
mod settings;
mod text;
mod view;
mod window;
pub use app::*;
pub use collection::*;
pub use compositor::*;
pub use control::*;
pub use drawing::*;
pub use help::*;
pub use input::*;
pub use layout::*;
pub use menu::*;
pub use navigation::*;
pub use objc2::MainThreadMarker;
pub use objc2::rc::Weak;
pub use objc2_core_foundation::{CGPoint, CGRect, CGSize};
pub use objc2_foundation::NSEdgeInsets as Insets;
pub use preview::*;
pub use settings::*;
pub use text::*;
pub use view::*;
pub use window::*;

#[derive(Clone, Copy)]
pub struct Ui {
    mtm: MainThreadMarker,
}
impl Ui {
    pub fn new(mtm: MainThreadMarker) -> Self { Self { mtm } }

    pub fn mtm(&self) -> MainThreadMarker { self.mtm }
}
