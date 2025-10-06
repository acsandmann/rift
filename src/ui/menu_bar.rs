// many ideas for how this works were taken from https://github.com/xiamaz/YabaiIndicator

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSGraphicsContext, NSImage, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use tracing::{debug, warn};

use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;

pub struct MenuIcon {
    status_item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
}

impl MenuIcon {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let status_item = {
            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

            if let Some(button) = status_item.button(mtm) {
                debug!("MenuIcon::new: got status item button");
                let title = NSString::from_str("rift");
                button.setTitle(&title);
                status_item.setLength(50.0);
                status_item.setVisible(true);
            } else {
                warn!("MenuIcon::new: could not get button from status item");
            }

            status_item
        };

        Self { status_item, mtm }
    }

    pub fn update(
        &mut self,
        _active_space: SpaceId,
        workspaces: Vec<WorkspaceData>,
        _active_workspace: Option<VirtualWorkspaceId>,
        _windows: Vec<WindowData>,
        show_all_workspaces: bool,
    ) {
        debug!("workspaces = {}, windows = {}", workspaces.len(), _windows.len());

        unsafe {
            if let Some(button) = self.status_item.button(self.mtm) {
                let ws_to_render: Vec<&WorkspaceData> = if show_all_workspaces {
                    workspaces.iter().collect()
                } else {
                    workspaces.iter().filter(|w| w.window_count > 0).collect()
                };

                if ws_to_render.is_empty() {
                    self.status_item.setVisible(false);
                    return;
                }

                let cell_w = 20.0f64;
                let cell_h = 15.0f64;
                let spacing = 4.0f64;
                let count = ws_to_render.len();
                let total_w =
                    (cell_w * count as f64) + (spacing * (count.saturating_sub(1) as f64));
                let total_h = cell_h;

                let canvas_rect =
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(total_w, total_h));
                let image: Retained<NSImage> =
                    Retained::from(NSImage::initWithSize(NSImage::alloc(), canvas_rect.size));
                // TODO: fix and use block based impl
                #[allow(deprecated)]
                image.lockFocus();

                for (i, ws) in ws_to_render.iter().enumerate() {
                    let x = i as f64 * (cell_w + spacing);
                    let cell_rect = NSRect::new(NSPoint::new(x, 0.0), NSSize::new(cell_w, cell_h));
                    let has_windows = ws.window_count > 0;
                    Self::draw_workspace_cell(&ws.windows, cell_rect, ws.is_active, has_windows);
                }

                #[allow(deprecated)]
                image.unlockFocus();
                image.setTemplate(true);

                let empty = NSString::from_str("");
                button.setTitle(&empty);
                button.setImage(Some(&*image));

                self.status_item.setLength(total_w);
                self.status_item.setVisible(true);
            } else {
                warn!("MenuIcon::update: could not get button from status item");
            }
        }
    }
}

impl MenuIcon {
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn draw_workspace_cell(
        windows: &[WindowData],
        rect: NSRect,
        is_active: bool,
        has_windows: bool,
    ) {
        let radius = 3.0f64;
        let border_w = 1.0f64;
        let content_inset = 2.0f64;

        let bg_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
        NSColor::blackColor().setStroke();
        bg_path.setLineWidth(border_w);

        let fill_alpha = if is_active {
            1.0
        } else if has_windows {
            0.45
        } else {
            0.0
        };
        if fill_alpha > 0.0 {
            let fill = NSColor::colorWithCalibratedWhite_alpha(0.0, fill_alpha);
            fill.setFill();
            bg_path.fill();
        }
        bg_path.stroke();

        if windows.is_empty() {
            return;
        }

        let min_x = windows.iter().map(|w| w.frame.origin.x).fold(f64::INFINITY, f64::min);
        let min_y = windows.iter().map(|w| w.frame.origin.y).fold(f64::INFINITY, f64::min);
        let max_x = windows
            .iter()
            .map(|w| w.frame.origin.x + w.frame.size.width)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = windows
            .iter()
            .map(|w| w.frame.origin.y + w.frame.size.height)
            .fold(f64::NEG_INFINITY, f64::max);

        let disp_w = (max_x - min_x).max(1.0);
        let disp_h = (max_y - min_y).max(1.0);

        let cx = rect.origin.x + content_inset;
        let cy = rect.origin.y + content_inset;
        let cw = (rect.size.width - 2.0 * content_inset).max(1.0);
        let ch = (rect.size.height - 2.0 * content_inset).max(1.0);

        let scaling = if disp_h > disp_w {
            disp_h / ch
        } else {
            disp_w / cw
        };
        let sf = 1.0 / scaling;

        let xoffset = if disp_h > disp_w {
            (cw - disp_w * sf) / 2.0
        } else {
            0.0
        } + cx;
        let yoffset = if disp_h > disp_w {
            0.0
        } else {
            (ch - disp_h * sf) / 2.0
        } + cy;

        for window in windows.iter().rev() {
            let wx = window.frame.origin.x - min_x;
            let wy_top = window.frame.origin.y - min_y + window.frame.size.height;
            let wy = disp_h - wy_top;
            let ww = window.frame.size.width;
            let wh = window.frame.size.height;

            let mut rx = xoffset + wx * sf;
            let mut ry = yoffset + wy * sf;
            let mut rw = (ww * sf).max(2.0);
            let mut rh = (wh * sf).max(2.0);

            let win_gap = 0.75f64;
            if rw > (2.0 + win_gap) {
                rx += win_gap / 2.0;
                rw -= win_gap;
            }
            if rh > (2.0 + win_gap) {
                ry += win_gap / 2.0;
                rh -= win_gap;
            }

            let win_rect = NSRect::new(NSPoint::new(rx, ry), NSSize::new(rw, rh));
            let win_path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(win_rect, 1.5, 1.5);

            NSColor::blackColor().setFill();
            win_path.fill();

            if let Some(ctx) = NSGraphicsContext::currentContext() {
                ctx.saveGraphicsState();
                ctx.setCompositingOperation(NSCompositingOperation::DestinationOut);
                win_path.setLineWidth(1.5);
                NSColor::blackColor().setStroke();
                win_path.stroke();
                ctx.restoreGraphicsState();
            } else {
                win_path.setLineWidth(1.5);
                NSColor::blackColor().setStroke();
                win_path.stroke();
            }
        }
    }
}

impl Drop for MenuIcon {
    fn drop(&mut self) {
        debug!("Removing menu bar icon");

        let status_bar = NSStatusBar::systemStatusBar();
        status_bar.removeStatusItem(&self.status_item);
    }
}
