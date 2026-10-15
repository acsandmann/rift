//! Embed the shipping cgs tab component; only the example window chrome is drawn here.
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

use super::*;
use crate::common::config::*;

const APPS: [(&str, &str); 3] = [
    ("Finder", "/System/Library/CoreServices/Finder.app"),
    ("Notes", "/System/Applications/Notes.app"),
    ("Terminal", "/System/Applications/Utilities/Terminal.app"),
];
struct ExampleStack {
    tabs: TabBar,
    surface: Retained<NSView>,
    apps: RefCell<Vec<usize>>,
    selected: Cell<usize>,
}
struct StackState {
    settings: RefCell<StackLineSettings>,
    stacks: RefCell<Vec<ExampleStack>>,
    labels: Vec<Retained<NSAttributedString>>,
    icons: [Image; 3],
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftSettingsStackPreview"]
    #[ivars = StackState]
    struct StackPreviewView;
    impl StackPreviewView {
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
        #[unsafe(method(viewDidChangeEffectiveAppearance))] fn appearance(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(layout))] fn arrange(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            let reserve = self.reserve();
            let settings = self.ivars().settings.borrow();
            for (i, stack) in self.ivars().stacks.borrow().iter().enumerate() {
                let horizontal = i == 0;
                let frame = reserve.bar(self.group(horizontal), horizontal);
                stack.tabs.set_frame(frame);
                stack.tabs.set_edge(match (horizontal, reserve.horiz, reserve.vert) {
                    (true, HorizontalPlacement::Top, _) => TabBarEdge::Top,
                    (true, HorizontalPlacement::Bottom, _) => TabBarEdge::Bottom,
                    (false, _, VerticalPlacement::Left) => TabBarEdge::Left,
                    (false, _, VerticalPlacement::Right) => TabBarEdge::Right,
                });
                stack.tabs.set_select_on_hover(settings.hover.selects_on_hover());
                stack.surface.setHidden(!settings.enabled || stack.apps.borrow().is_empty());

            }
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(drawRect:))] fn draw(&self, _: CGRect) { self.draw_windows(); }
    }
);
pub(super) struct StackPreview(Retained<StackPreviewView>);
impl NativeView for StackPreview {
    fn ns_view(&self) -> &NSView { &self.0 }
}
impl StackPreview {
    pub(super) fn new(ui: &Ui) -> Self {
        let font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightMedium });
        let color = NSColor::labelColor();
        let attrs = NSDictionary::from_slices(
            &[unsafe { NSFontAttributeName }, unsafe {
                NSForegroundColorAttributeName
            }],
            &[&*font as &objc2::runtime::AnyObject, &*color],
        );
        let labels = APPS
            .iter()
            .map(|(title, _)| unsafe {
                NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(title),
                    Some(&attrs),
                )
            })
            .collect();
        let view: Retained<StackPreviewView> = unsafe {
            msg_send![
                super(StackPreviewView::alloc(ui.mtm()).set_ivars(StackState {
                    settings: RefCell::new(StackLineSettings::default()),
                    stacks: RefCell::new(Vec::with_capacity(2)),
                    labels,
                    icons: APPS.map(|(_, path)| {
                        NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path))
                    }),
                })),
                init
            ]
        };
        for i in 0..2 {
            let weak = objc2::rc::Weak::new(&*view);
            let tabs = TabBar::embedded(ui).on_action(move |action| {
                if let Some(view) = weak.load() {
                    view.act(i, action);
                }
            });
            let surface = tabs.ns_view().retain();
            surface.setTranslatesAutoresizingMaskIntoConstraints(true);
            surface.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
            surface.setAccessibilityLabel(Some(&NSString::from_str(if i == 0 {
                "Horizontal stack example"
            } else {
                "Vertical stack example"
            })));
            view.addSubview(&surface);
            view.ivars().stacks.borrow_mut().push(ExampleStack {
                tabs,
                surface,
                apps: RefCell::new(vec![]),
                selected: Cell::new(0),
            });
        }
        let preview = Self(view);
        preview.reset();
        preview
    }

    pub(super) fn update(&self, settings: &StackLineSettings) {
        let geometry = {
            let old = self.0.ivars().settings.borrow();
            if *old == *settings {
                return;
            }
            old.enabled != settings.enabled
                || old.thickness != settings.thickness
                || old.spacing != settings.spacing
                || old.horiz_placement != settings.horiz_placement
                || old.vert_placement != settings.vert_placement
        };
        *self.0.ivars().settings.borrow_mut() = settings.clone();
        for stack in self.0.ivars().stacks.borrow().iter() {
            stack.tabs.set_select_on_hover(settings.hover.selects_on_hover());
            stack.surface.setHidden(!settings.enabled || stack.apps.borrow().is_empty());
        }
        if geometry {
            self.0.setNeedsLayout(true);
            self.0.setNeedsDisplay(true);
        }
    }

    pub(super) fn reset(&self) {
        for (i, stack) in self.0.ivars().stacks.borrow().iter().enumerate() {
            *stack.apps.borrow_mut() = if i == 0 { vec![0, 1, 2] } else { vec![0, 2] };
            stack.selected.set(0);
            self.0.refresh(stack);
        }
        self.0.setNeedsLayout(true);
        self.0.setNeedsDisplay(true);
    }
}
impl StackPreviewView {
    fn refresh(&self, stack: &ExampleStack) {
        stack.tabs.set_tabs(
            stack
                .apps
                .borrow()
                .iter()
                .map(|&i| TabBarItem {
                    title: APPS[i].0.into(),
                    icon: Some(self.ivars().icons[i].clone()),
                })
                .collect(),
            stack.selected.get(),
        );
    }

    fn act(&self, group: usize, action: TabBarAction) {
        let stacks = self.ivars().stacks.borrow();
        let stack = &stacks[group];
        let count = stack.apps.borrow().len();
        match action {
            TabBarAction::Select(i) if i < count => {
                stack.selected.set(i);
            }
            TabBarAction::Close(i) if i < count => {
                stack.apps.borrow_mut().remove(i);
                let selected = stack.selected.get();
                stack.selected.set(if i < selected {
                    selected - 1
                } else {
                    selected.min(count.saturating_sub(2))
                });
            }
            _ => return,
        }
        self.refresh(stack);
        stack
            .surface
            .setHidden(!self.ivars().settings.borrow().enabled || stack.apps.borrow().is_empty());
        self.setNeedsDisplay(true);
    }

    fn reserve(&self) -> StackLineReserve {
        let s = self.ivars().settings.borrow();
        StackLineReserve {
            thickness: if s.enabled { s.thickness } else { 0.0 },
            spacing: s.spacing,
            horiz: s.horiz_placement,
            vert: s.vert_placement,
        }
    }

    fn group(&self, horizontal: bool) -> CGRect {
        let w = self.bounds().size.width;
        if horizontal {
            rect(16.0, 16.0, (w - 52.0) * 0.67, 248.0)
        } else {
            rect(36.0 + (w - 52.0) * 0.67, 16.0, (w - 52.0) * 0.33, 248.0)
        }
    }

    fn draw_windows(&self) {
        round(self.bounds(), 10.0, &NSColor::underPageBackgroundColor());
        let state = self.ivars();
        for (i, stack) in state.stacks.borrow().iter().enumerate() {
            let window = self.reserve().inset(self.group(i == 0), i == 0);
            let apps = stack.apps.borrow();
            for depth in (0..apps.len()).rev() {
                let d = depth as f64 * 3.0;
                let r = rect(
                    window.origin.x + d,
                    window.origin.y + d,
                    window.size.width - 6.0,
                    window.size.height - 6.0,
                );
                round(
                    rect(r.origin.x, r.origin.y + 2.0, r.size.width, r.size.height),
                    7.0,
                    &NSColor::shadowColor().colorWithAlphaComponent(0.12),
                );
                round(r, 7.0, &NSColor::windowBackgroundColor());
                if r.size.width < 28.0 || r.size.height < 28.0 {
                    continue;
                }
                NSColor::separatorColor().setStroke();
                let outline = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 7.0, 7.0);
                outline.setLineWidth(0.5);
                outline.stroke();
                if depth != 0 {
                    continue;
                }
                for (i, color) in [
                    NSColor::systemRedColor(),
                    NSColor::systemYellowColor(),
                    NSColor::systemGreenColor(),
                ]
                .iter()
                .enumerate()
                {
                    round(
                        rect(r.origin.x + 10.0 + i as f64 * 13.0, r.origin.y + 11.0, 8.0, 8.0),
                        4.0,
                        color,
                    );
                }
                if let Some(&app) = apps.get(stack.selected.get()) {
                    state.labels[app]
                        .drawAtPoint(CGPoint::new(r.origin.x + 12.0, r.origin.y + 36.0));
                }
                for line in 0..4 {
                    if 65.0 + line as f64 * 13.0 + 4.0 > r.size.height - 12.0 {
                        break;
                    }
                    round(
                        rect(
                            r.origin.x + 12.0,
                            r.origin.y + 65.0 + line as f64 * 13.0,
                            (r.size.width - 28.0) * (if line == 3 { 0.6 } else { 1.0 }),
                            4.0,
                        ),
                        2.0,
                        &NSColor::tertiaryLabelColor().colorWithAlphaComponent(0.18),
                    );
                }
            }
        }
    }
}
pub(super) fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w.max(0.0), h.max(0.0)))
}
pub(super) fn round(r: CGRect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius).fill();
}
