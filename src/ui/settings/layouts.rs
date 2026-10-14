//! Layout configuration and independent, lazy shared-settings disclosures.
use std::cell::OnceCell;

use objc2::Message;

use super::*;
use crate::common::config::{
    LayoutMode, MasterStackSide, ScrollingAlignment, StackDefaultOrientation,
};
use crate::layout_engine::Orientation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Layout(LayoutMode),
    Spacing,
    Displays,
}
impl Target {
    fn index(self) -> usize {
        match self {
            Self::Layout(mode) => mode.config_choice_index(),
            Self::Spacing => 6,
            Self::Displays => 7,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Layout(mode) => pages::layout_title(mode),
            Self::Spacing => "Spacing",
            Self::Displays => "Displays",
        }
    }
}
/// Canvas size of each layout tile’s illustration.
const TILE: CGSize = CGSize::new(64.0, 40.0);

pub(super) struct Inspector {
    ui: Ui,
    model: Weak<Model>,
    root: Page,
    choices: Vec<Rc<Button>>,
    title: Label,
    summary: Caption,
    /// Set as Default beside another layout’s title, or a quiet confirmation on the default.
    set_default: Rc<Button>,
    is_default: Rc<HStack>,
    /// Restore Defaults in the page footer, for the edited layout.
    restore_button: Rc<Button>,
    /// Each tile’s context menu, with its Set as Default item.
    tile_menus: Vec<(Rc<Menu>, WeakMenuItem)>,
    error: Rc<ValidationMessage>,
    host: Rc<PageHost>,
    shared_hosts: [Rc<PageHost>; 2],
    disclosures: [Rc<Disclosure>; 2],
    selected: Cell<LayoutMode>,
    /// The (edited, default) pair the tiles and header currently show.
    shown: Cell<Option<(LayoutMode, LayoutMode)>>,
    pages: RefCell<[Option<Page>; 8]>,
}
impl Inspector {
    pub fn new(ui: Ui, model: &Rc<Model>) -> Rc<Self> {
        let selected = model.source.borrow().settings.layout.mode;
        let inspector = Rc::new_cyclic(|weak: &Weak<Self>| {
            let mut choices = Vec::new();
            let mut tile_menus = Vec::new();
            for &(title, mode) in pages::layouts() {
                let owner = weak.clone();
                let preview = LayoutPreview::new(&ui, TILE);
                let button = Rc::new(
                    Button::new(&ui, title).layout_illustration(preview).on_click(move || {
                        if let Some(inspector) = owner.upgrade() {
                            inspector.select_target(Target::Layout(mode));
                        }
                    }),
                );
                // A little air between the illustration and its name, and room for the
                // default marker below.
                button.ns_button().setImage(Some(&objc2_app_kit::NSImage::initWithSize(
                    <objc2_app_kit::NSImage as objc2::AnyThread>::alloc(),
                    CGSize::new(TILE.width, TILE.height + 4.0),
                )));
                button.height(TILE.height + 46.0);
                button.accessibility_label(&format!("Configure {title}"));
                // Right-click a tile to make it the default or restore its options.
                let owner = weak.clone();
                let make_default =
                    MenuItem::new(&ui, "Set as Default Layout").on_click(move || {
                        if let Some(inspector) = owner.upgrade() {
                            inspector.set_default(mode);
                        }
                    });
                let default_item = make_default.weak();
                let mut menu = Menu::new(&ui).item(make_default);
                if mode != LayoutMode::Floating {
                    let owner = weak.clone();
                    menu = menu.separator().item(
                        MenuItem::new(&ui, &format!("Restore {title} Defaults…")).on_click(
                            move || {
                                if let Some(inspector) = owner.upgrade() {
                                    inspector.confirm_restore(mode);
                                }
                            },
                        ),
                    );
                }
                let menu = Rc::new(menu);
                // SAFETY: the menu is kept alive with the tile in `tile_menus`.
                unsafe { button.ns_button().setMenu(Some(menu.ns_menu())) };
                tile_menus.push((menu, default_item));
                choices.push(button);
            }
            let picker = choices.iter().fold(
                HStack::new(&ui)
                    .spacing(4.0)
                    .distribution(objc2_app_kit::NSStackViewDistribution::FillEqually),
                |row, button| row.push(button.clone()),
            );
            let title = Label::new(&ui, "").font(&Font::body_emphasized());
            let summary = Caption::new(&ui, "").max_lines(2);
            let owner = weak.clone();
            let set_default = Rc::new(Button::new(&ui, "Set as Default").on_click(move || {
                if let Some(inspector) = owner.upgrade() {
                    inspector.set_default(inspector.selected.get());
                }
            }));
            set_default.control_size(objc2_app_kit::NSControlSize::Small);
            set_default.ns_button().setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
            let is_default = Rc::new(
                ImageView::symbol(&ui, "checkmark.circle.fill")
                    .into_iter()
                    .fold(HStack::new(&ui).spacing(4.0), |row, check| {
                        check.ns_image_view().setContentTintColor(Some(
                            &objc2_app_kit::NSColor::secondaryLabelColor(),
                        ));
                        row.push(check)
                    })
                    .push(Caption::new(&ui, "Default Layout")),
            );
            // The default action shares the title’s line, at the editor’s trailing edge.
            let title_row = HStack::new(&ui)
                .push(title.ns_view().retain())
                .push(Spacer::new(&ui))
                .push(set_default.clone())
                .push(is_default.clone());
            title_row
                .ns_view()
                .heightAnchor()
                .constraintGreaterThanOrEqualToConstant(set_default.ns_view().fittingSize().height)
                .setActive(true);
            let header =
                VStack::new(&ui).spacing(0.0).push(title_row).push(summary.ns_view().retain());
            let host = Rc::new(PageHost::content_sized(&ui));
            let error = Rc::new(ValidationMessage::new(&ui));
            let editor = VStack::new(&ui)
                .spacing(12.0)
                .push(picker)
                .push(Divider::new(&ui))
                .push(header)
                .push(host.clone())
                .push(error.clone());
            let shared_hosts = std::array::from_fn(|_| Rc::new(PageHost::content_sized(&ui)));
            let disclosures = std::array::from_fn(|index| {
                let owner = weak.clone();
                let target = [Target::Spacing, Target::Displays][index];
                Rc::new(
                    Disclosure::new(
                        &ui,
                        target.title(),
                        View::new(&ui).content(shared_hosts[index].clone(), Insets {
                            top: 0.0,
                            left: 12.0,
                            bottom: 12.0,
                            right: 12.0,
                        }),
                    )
                    .header_insets(Insets {
                        top: 12.0,
                        left: 12.0,
                        bottom: 12.0,
                        right: 12.0,
                    })
                    .compact()
                    .trailing_indicator(&ui)
                    .on_toggle(move |expanded| {
                        owner
                            .upgrade()
                            .is_some_and(|inspector| inspector.set_shared(target, expanded))
                    }),
                )
            });
            let mut form = FormBuilder::new(ui, model);
            let owner = weak.clone();
            let restore_button =
                Rc::new(Button::new(&ui, "Restore Defaults…").on_click(move || {
                    if let Some(inspector) = owner.upgrade() {
                        inspector.confirm_restore(inspector.selected.get());
                    }
                }));
            restore_button.ns_button().setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
            let position = Section::form(&ui, "").row(pages::window_position(&mut form));
            pages::align_form(position.ns_view());
            let inset = |content: Box<dyn NativeView>, vertical: f64| {
                View::new(&ui).content(content, Insets {
                    top: vertical,
                    left: 12.0,
                    bottom: vertical,
                    right: 12.0,
                })
            };
            let shared = VStack::new(&ui)
                .spacing(0.0)
                .push(inset(Box::new(position), 8.0))
                .push(inset(Box::new(Divider::new(&ui)), 0.0))
                .push(disclosures[0].clone())
                .push(inset(Box::new(Divider::new(&ui)), 0.0))
                .push(disclosures[1].clone());
            let root = form.finish(
                SettingsPage::new(&ui, "")
                    .section(GroupBox::with_insets(&ui, editor, cgs::uniform_insets(12.0)))
                    .section(
                        Section::new(&ui, "Shared settings").content(GroupBox::with_insets(
                            &ui,
                            shared,
                            cgs::uniform_insets(0.0),
                        )),
                    )
                    // A preferences footer: restore the edited layout, and help.
                    .section(
                        HStack::new(&ui)
                            .push(restore_button.clone())
                            .push(Spacer::new(&ui))
                            .push(layout_help(&ui)),
                    ),
            );
            Self {
                ui,
                model: Rc::downgrade(model),
                root,
                choices,
                title,
                summary,
                set_default,
                is_default,
                restore_button,
                tile_menus,
                error,
                host,
                shared_hosts,
                disclosures,
                selected: Cell::new(selected),
                shown: Cell::new(None),
                pages: RefCell::new(std::array::from_fn(|_| None)),
            }
        });
        inspector.select_target(Target::Layout(selected));
        inspector
    }

    /// Make `mode` the default for workspaces without their own layout.
    fn set_default(&self, mode: LayoutMode) {
        if self.shown.get().is_some_and(|(_, default)| default == mode) {
            return;
        }
        Model::submit(
            &self.model,
            Box::new(move |source| {
                source.settings.layout.mode = mode;
                Ok(())
            }),
            Rc::downgrade(&self.error),
        );
    }

    fn confirm_restore(&self, mode: LayoutMode) {
        if !finish_editing(self.host.ns_view()) {
            return;
        }
        let Some(window) = self.root.view.ns_view().window() else {
            return;
        };
        let title = pages::layout_title(mode);
        let model = self.model.clone();
        let error = Rc::downgrade(&self.error);
        Alert::new(
            &self.ui,
            &format!("Restore {title} defaults?"),
            &format!(
                "{title}’s options return to their original values. Your default layout, spacing, and display settings don’t change."
            ),
        )
        .button("Restore Defaults")
        .button("Cancel")
        .show_sheet(&window, move |response| {
            if response == objc2_app_kit::NSAlertFirstButtonReturn {
                restore(&model, mode, error);
            }
        });
    }

    pub fn page(self: &Rc<Self>) -> Page {
        let weak = Rc::downgrade(self);
        Page {
            view: self.clone(),
            sync: vec![Box::new(move |_| {
                if let Some(inspector) = weak.upgrade() {
                    inspector.synchronize();
                }
            })],
            header: None,
            synced_revision: Cell::new(None),
        }
    }

    pub fn select_target(&self, target: Target) {
        let Target::Layout(mode) = target else {
            self.set_shared(target, true);
            return;
        };
        let Some(model) = self.model.upgrade() else {
            return;
        };
        if !finish_editing(self.host.ns_view()) {
            self.update_selection();
            return;
        }
        self.selected.set(mode);
        self.update_selection();
        let index = mode.config_choice_index();
        if self.pages.borrow()[index].is_none() {
            self.pages.borrow_mut()[index] = Some(pages::layout_options(self.ui, &model, mode));
        }
        let pages = self.pages.borrow();
        let page = pages[index].as_ref().unwrap();
        page.synchronize(&model);
        self.host.set_cached_page(page.view.clone());
        let floating = mode == LayoutMode::Floating;
        self.host.set_hidden(floating);
        self.restore_button.set_hidden(floating);
        self.update_previews(&model.source.borrow());
    }

    fn set_shared(&self, target: Target, expanded: bool) -> bool {
        let index = target.index();
        let host = &self.shared_hosts[index - 6];
        if !finish_editing(host.ns_view()) {
            return false;
        }
        if expanded {
            let other = 1 - (index - 6);
            if !finish_editing(self.shared_hosts[other].ns_view()) {
                return false;
            }
            self.disclosures[other].set_expanded(false);
            let Some(model) = self.model.upgrade() else {
                return false;
            };
            if self.pages.borrow()[index].is_none() {
                self.pages.borrow_mut()[index] = Some(match target {
                    Target::Spacing => pages::layout_spacing(self.ui, &model),
                    Target::Displays => pages::layout_displays(self.ui, &model),
                    _ => unreachable!(),
                });
            }
            let pages = self.pages.borrow();
            let page = pages[index].as_ref().unwrap();
            page.synchronize(&model);
            host.set_cached_page(page.view.clone());
            // The disclosure expands inside an animation. Lay a newly mounted editor out at
            // its final width first, with animation off, so only the section’s height
            // animates rather than every new control growing from an empty frame.
            objc2_app_kit::NSAnimationContext::beginGrouping();
            let context = objc2_app_kit::NSAnimationContext::currentContext();
            context.setDuration(0.0);
            context.setAllowsImplicitAnimation(false);
            let view = host.ns_view();
            let width = self.disclosures[index - 6].ns_view().frame().size.width - 24.0;
            view.setFrameSize(CGSize::new(width, view.fittingSize().height));
            view.layoutSubtreeIfNeeded();
            objc2_app_kit::NSAnimationContext::endGrouping();
        }
        self.disclosures[index - 6].set_expanded(expanded);
        true
    }

    fn update_selection(&self) {
        for (index, button) in self.choices.iter().enumerate() {
            let selected = index == self.selected.get().config_choice_index();
            button.ns_button().setState(if selected { 1 } else { 0 });
            button.layout_preview().unwrap().set_selected(selected);
        }
    }

    /// Name the tiles and the editor header for the edited and default layouts.
    fn update_header(&self, default: LayoutMode) {
        let mode = self.selected.get();
        if self.shown.replace(Some((mode, default))) == Some((mode, default)) {
            return;
        }
        for (button, &(title, choice)) in self.choices.iter().zip(pages::layouts()) {
            tile_title(button.ns_button(), title, choice == mode, choice == default);
        }
        let title = pages::layout_title(mode);
        self.title.set_text(title);
        self.summary.set_text(LayoutMode::CONFIG_CHOICES[mode.config_choice_index()].1);
        self.set_default.set_hidden(mode == default);
        self.set_default.accessibility_label(&format!("Set {title} as Default"));
        self.is_default.set_hidden(mode != default);
        self.restore_button.set_title(&format!("Restore {title} Defaults…"));
        for ((_, item), &(_, choice)) in self.tile_menus.iter().zip(pages::layouts()) {
            item.set_checked(choice == default);
            item.set_enabled(choice != default);
        }
    }

    fn synchronize(&self) {
        let Some(model) = self.model.upgrade() else {
            return;
        };
        self.root.synchronize(&model);
        let pages = self.pages.borrow();
        if let Some(page) = &pages[self.selected.get().config_choice_index()] {
            page.synchronize(&model);
        }
        for (index, host) in self.shared_hosts.iter().enumerate() {
            if !host.ns_view().isHiddenOrHasHiddenAncestor()
                && let Some(page) = &pages[index + 6]
            {
                page.synchronize(&model);
            }
        }
        self.update_previews(&model.source.borrow());
    }

    fn update_previews(&self, source: &ConfigSource) {
        self.update_header(source.settings.layout.mode);
        for (button, (_, mode)) in self.choices.iter().zip(pages::layouts().iter().copied()) {
            let preview = button.layout_preview().unwrap();
            preview.set_windows(
                &sample(source, mode, preview.canvas_size()),
                PreviewAnimation::Immediate,
            );
        }
    }

    pub fn refresh_displays(&self) {
        self.pages.borrow_mut()[7] = None;
        if !self.shared_hosts[1].ns_view().isHiddenOrHasHiddenAncestor() {
            self.set_shared(Target::Displays, true);
        }
    }
}
impl NativeView for Inspector {
    fn ns_view(&self) -> &objc2_app_kit::NSView { self.root.view.ns_view() }
}

/// Four deterministic sample windows, in canvas points. No window-system queries,
/// parsing, heap allocation, or configuration mutations are needed for this illustration.
fn sample(source: &ConfigSource, mode: LayoutMode, size: CGSize) -> [PreviewWindow; 4] {
    // Keep all four native identities across selections; unused sample windows have zero area.
    let mut windows = std::array::from_fn(|id| PreviewWindow::new(id as u64, CGRect::ZERO));
    let mut count = 0;
    // Tiles are the reference size; larger previews keep their proportions.
    let scale = size.height / TILE.height;
    let margin = 5.0 * scale.sqrt();
    let mut add = |x, y, width, height| {
        windows[count] = PreviewWindow::new(
            count as u64,
            CGRect::new(CGPoint::new(x + margin, y + margin), CGSize::new(width, height)),
        );
        count += 1;
    };
    let (w, h, gap) = (
        size.width - 2.0 * margin,
        size.height - 2.0 * margin,
        3.0 * scale.sqrt(),
    );
    let layout = &source.settings.layout;
    match mode {
        LayoutMode::Traditional | LayoutMode::Bsp => {
            if mode == LayoutMode::Bsp
                && let Some(ratio) = layout.bsp.single_window_aspect_ratio
            {
                let width = w.min(h * ratio);
                let height = h.min(w / ratio);
                add((w - width) / 2.0, (h - height) / 2.0, width, height);
            } else {
                let ratio = if mode == LayoutMode::Traditional && !layout.traditional.equalize_nodes
                {
                    0.6
                } else {
                    0.5
                };
                let left = (w - gap) * ratio;
                if mode == LayoutMode::Bsp {
                    add(0.0, 0.0, left, (h - gap) / 2.0);
                    add(0.0, (h + gap) / 2.0, left, (h - gap) / 2.0);
                    add(left + gap, 0.0, w - left - gap, h);
                } else {
                    add(0.0, 0.0, left, h);
                    add(left + gap, 0.0, w - left - gap, (h - gap) / 2.0);
                    add(left + gap, (h + gap) / 2.0, w - left - gap, (h - gap) / 2.0);
                }
            }
        }
        LayoutMode::Stack => {
            let vertical = matches!(
                layout.stack.default_orientation,
                StackDefaultOrientation::Vertical | StackDefaultOrientation::Perpendicular
            );
            // Cascaded cards: each window further back peeks out by the offset and
            // is narrower, so the stack reads as depth rather than stripes.
            let along = if vertical { h } else { w };
            let offset = (layout.stack.stack_offset * 0.15 * scale).clamp(3.0, along / 5.0);
            let inset = if vertical { w } else { h } / 12.0;
            for i in 0..3 {
                let depth = (2 - i) as f64;
                let (x, y) = (depth * inset, i as f64 * offset);
                let (across, length) = (2.0 * depth * inset, 2.0 * offset);
                if vertical {
                    add(x, y, w - across, h - length);
                } else {
                    add(y, x, w - length, h - across);
                }
            }
        }
        LayoutMode::MasterStack => {
            let settings = &layout.master_stack;
            let sideways = matches!(
                settings.master_side,
                MasterStackSide::Left | MasterStackSide::Right
            );
            let extent = if sideways { w } else { h };
            let master = (extent - gap) * settings.master_ratio;
            let first = matches!(
                settings.master_side,
                MasterStackSide::Left | MasterStackSide::Top
            );
            let masters = settings.master_count.clamp(1, 3);
            let natural = if sideways {
                Orientation::Vertical
            } else {
                Orientation::Horizontal
            };
            for (start, length, n, arrangement) in [
                (
                    if first { 0.0 } else { extent - master },
                    master,
                    masters,
                    settings.master_arrangement.unwrap_or(natural),
                ),
                (
                    if first { master + gap } else { 0.0 },
                    extent - master - gap,
                    4 - masters,
                    settings.stack_arrangement.unwrap_or(natural),
                ),
            ] {
                let (x, y, width, height) = if sideways {
                    (start, 0.0, length, h)
                } else {
                    (0.0, start, w, length)
                };
                let vertical = arrangement == Orientation::Vertical;
                let step = (if vertical { height } else { width } + gap) / n as f64;
                for i in 0..n {
                    add(
                        x + if vertical { 0.0 } else { i as f64 * step },
                        y + if vertical { i as f64 * step } else { 0.0 },
                        if vertical { width } else { step - gap },
                        if vertical { step - gap } else { height },
                    );
                }
            }
        }
        LayoutMode::Scrolling => {
            let settings = &layout.scrolling;
            let column = (w * settings.column_width_ratio).max(8.0);
            let anchor = match settings.alignment {
                ScrollingAlignment::Left => 0.0,
                ScrollingAlignment::Center => (w - column) / 2.0,
                ScrollingAlignment::Right => w - column,
            };
            for i in -1..=1 {
                let x = anchor + i as f64 * (column + gap);
                let left = x.max(0.0);
                let right = (x + column).min(w);
                add(left, 0.0, (right - left).max(0.0), h);
            }
        }
        LayoutMode::Floating => {
            add(0.0, h * 0.1, w * 0.55, h * 0.65);
            add(w * 0.4, h * 0.35, w * 0.55, h * 0.55);
        }
    }
    windows
}

/// Restore a layout’s options, keeping the default layout and display overrides.
pub(super) fn restore(model: &Weak<Model>, mode: LayoutMode, error: Weak<ValidationMessage>) {
    Model::submit(
        model,
        Box::new(move |source| {
            let layout = &mut source.settings.layout;
            match mode {
                LayoutMode::Traditional => layout.traditional = Default::default(),
                LayoutMode::Bsp => layout.bsp = Default::default(),
                LayoutMode::Stack => layout.stack = Default::default(),
                LayoutMode::MasterStack => layout.master_stack = Default::default(),
                LayoutMode::Scrolling => {
                    let displays = std::mem::take(&mut layout.scrolling.per_display);
                    layout.scrolling = Default::default();
                    layout.scrolling.per_display = displays;
                }
                LayoutMode::Floating => {}
            }
            Ok(())
        }),
        error,
    );
}

/// A tile’s two-line title: the layout name, emphasized while edited, then a quiet
/// marker on the default.
/// Every tile reserves the second line so names stay aligned.
fn tile_title(button: &objc2_app_kit::NSButton, title: &str, selected: bool, default: bool) {
    use objc2::AnyThread;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMutableParagraphStyle,
        NSParagraphStyleAttributeName, NSTextAlignment,
    };
    use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString, NSString};
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setParagraphSpacing(1.0);
    let line = |text: &str, font: &NSFont, color: &objc2_app_kit::NSColor| unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&NSDictionary::from_slices(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[
                    font as &AnyObject,
                    color as &AnyObject,
                    &*paragraph as &AnyObject,
                ],
            )),
        )
    };
    let text = NSMutableAttributedString::new();
    text.appendAttributedString(&line(
        &format!("{title}\n"),
        &*if selected {
            NSFont::systemFontOfSize_weight(NSFont::smallSystemFontSize(), unsafe {
                objc2_app_kit::NSFontWeightSemibold
            })
        } else {
            Font::caption()
        },
        &objc2_app_kit::NSColor::labelColor(),
    ));
    text.appendAttributedString(&line(
        if default { "Default" } else { " " },
        &NSFont::systemFontOfSize_weight(NSFont::labelFontSize() - 1.0, unsafe {
            objc2_app_kit::NSFontWeightMedium
        }),
        &objc2_app_kit::NSColor::secondaryLabelColor(),
    ));
    button.setAttributedTitle(&text);
}

/// Let AppKit commit or validate the active field before its inspector is hidden.
fn finish_editing(view: &objc2_app_kit::NSView) -> bool {
    if view.isHidden() {
        return true;
    }
    if view
        .downcast_ref::<objc2_app_kit::NSControl>()
        .is_some_and(|control| control.currentEditor().is_some())
    {
        return view.window().is_none_or(|window| window.makeFirstResponder(None));
    }
    view.subviews().iter().all(|child| finish_editing(&child))
}

fn layout_help(ui: &Ui) -> HelpButton {
    let button = HelpButton::new(ui);
    button.accessibility_label("Layouts help");
    let anchor = objc2::rc::Weak::new(button.ns_button());
    let ui = *ui;
    let popover = OnceCell::new();
    button.on_click(move || {
        let help = popover.get_or_init(|| {
            let content = VStack::new(&ui).spacing(10.0)
                .push(SectionTitle::new(&ui, "Layouts"))
                .push(WrappingLabel::new(&ui, "Workspaces without their own layout use the one marked Default. Choosing a layout only shows its options. Use Set as Default, or right-click a layout, to change the default."))
                .push(WrappingLabel::new(&ui, "Shared settings apply to every layout. A layout’s own New window position overrides the shared one."))
                .push(Link::new(&ui, "Layout documentation", "https://acsandmann.github.io/rift-docs/layouts/"));
            content.width(320.0);
            HelpPopover::with_content(&ui, content)
        });
        if let Some(anchor) = anchor.load() { help.show(&anchor.into_super().into_super()); }
    })
}
