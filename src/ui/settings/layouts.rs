//! Layout configuration and independent, lazy shared-settings disclosures.
use std::cell::OnceCell;

use objc2::Message;
use objc2_app_kit::NSAccessibility;

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
/// Canvas size of each layout tile’s illustration.
const TILE: CGSize = CGSize::new(64.0, 40.0);
/// Cache slots after the six layouts: Spacing, then Display overrides.
const SHARED: usize = 6;

/// One page answers, top to bottom: which layout is the default (the toolbar popup),
/// which is being configured (the gallery’s selection), its own settings, the settings
/// every layout shares, and what changes on a particular display.
pub(super) struct Inspector {
    ui: Ui,
    model: Weak<Model>,
    root: Page,
    tiles: Vec<Rc<LayoutTile>>,
    /// The default layout, shown in the toolbar beside the page title.
    default_control: Rc<HStack>,
    default_popup: Rc<Popup>,
    title: Label,
    summary: Caption,
    actions: Rc<Popup>,
    /// Use as Default in the selected layout’s actions and each tile’s context menu.
    default_items: Vec<WeakMenuItem>,
    restore_item: WeakMenuItem,
    _tile_menus: Vec<Menu>,
    error: Rc<ValidationMessage>,
    host: Rc<PageHost>,
    floating: Rc<HStack>,
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
            let mut tiles = Vec::new();
            let mut default_items = Vec::new();
            let mut tile_menus = Vec::new();
            for &(title, mode) in pages::layouts() {
                let owner = weak.clone();
                let tile = LayoutTile::new(&ui, title, TILE).on_click(move || {
                    if let Some(inspector) = owner.upgrade() {
                        inspector.select_target(Target::Layout(mode));
                    }
                });
                tile.ns_button()
                    .setAccessibilityHelp(Some(&objc2_foundation::NSString::from_str(&format!(
                        "Shows {title}’s settings. Doesn’t change the default layout."
                    ))));
                // Right-click a tile to make it the default or restore its settings.
                let (menu, item, _) = layout_menu(&ui, weak, Some(mode));
                // SAFETY: the menu is kept alive with the tile in `tile_menus`.
                unsafe { tile.ns_button().setMenu(Some(menu.ns_menu())) };
                default_items.push(item);
                tile_menus.push(menu);
                tiles.push(Rc::new(tile));
            }
            let gallery = tiles.iter().fold(
                HStack::new(&ui)
                    .spacing(4.0)
                    .distribution(objc2_app_kit::NSStackViewDistribution::FillEqually),
                |row, tile| row.push(tile.clone()),
            );
            let group = gallery.ns_view();
            group.setAccessibilityElement(true);
            group.setAccessibilityRole(Some(unsafe {
                objc2_app_kit::NSAccessibilityRadioGroupRole
            }));
            group.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(
                "Layout to configure",
            )));

            let owner = weak.clone();
            let default_popup = Rc::new(
                Popup::new(&ui)
                    .items(pages::layouts().iter().map(|(title, _)| *title))
                    .toolbar_style()
                    .on_change(move |index| {
                        if let (Some(inspector), Some(mode)) =
                            (owner.upgrade(), LayoutMode::from_config_choice(index))
                        {
                            inspector.set_default(mode);
                        }
                    }),
            );
            default_popup.accessibility_label("Default layout");
            default_popup
                .tooltip("The layout new workspaces use, unless a workspace chooses its own.");
            let default_control = Rc::new(
                HStack::new(&ui)
                    .spacing(6.0)
                    .push(SecondaryLabel::single_line(&ui, "Default layout"))
                    .push(default_popup.clone()),
            );
            default_control.accessibility_label("Default layout");

            let title = Label::new(&ui, "").font(&Font::title3());
            let summary = Caption::new(&ui, "").max_lines(2);
            let (menu, use_as_default, restore_item) = layout_menu(&ui, weak, None);
            let actions = Rc::new(Popup::actions(&ui, "Layout actions", menu));
            let heading = HStack::new(&ui)
                .alignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline)
                .push(
                    VStack::new(&ui)
                        .spacing(2.0)
                        .push(title.ns_view().retain())
                        .push(summary.ns_view().retain()),
                )
                .push(Spacer::new(&ui))
                .push(actions.clone());
            let host = Rc::new(PageHost::content_sized(&ui));
            let floating = Rc::new(floating_note(&ui));
            let error = Rc::new(ValidationMessage::new(&ui));
            let editor = VStack::new(&ui)
                .spacing(14.0)
                .push(gallery)
                .push(Divider::new(&ui))
                .push(heading)
                .push(host.clone())
                .push(floating.clone())
                .push(error.clone());

            let mut form = FormBuilder::new(ui, model);
            let shared_hosts = std::array::from_fn(|_| Rc::new(PageHost::content_sized(&ui)));
            let disclosures = std::array::from_fn(|index| {
                let owner = weak.clone();
                let target = [Target::Spacing, Target::Displays][index];
                Rc::new(
                    Disclosure::new(
                        &ui,
                        ["Spacing", "Display overrides"][index],
                        View::new(&ui).content(shared_hosts[index].clone(), Insets {
                            top: 2.0,
                            left: 12.0,
                            bottom: 14.0,
                            right: 12.0,
                        }),
                    )
                    .header_insets(cgs::uniform_insets(12.0))
                    .compact()
                    .trailing_indicator(&ui)
                    .summary(&ui)
                    .on_toggle(move |expanded| {
                        owner
                            .upgrade()
                            .is_some_and(|inspector| inspector.set_shared(target, expanded))
                    }),
                )
            });
            let weak_disclosures = disclosures.each_ref().map(Rc::downgrade);
            let env = Rc::downgrade(&model.env);
            form.sync.push(Box::new(move |source| {
                let summaries = [
                    spacing_summary(source),
                    display_summary(source, env.upgrade().as_deref()),
                ];
                for (disclosure, summary) in weak_disclosures.iter().zip(summaries) {
                    if let Some(disclosure) = disclosure.upgrade() {
                        disclosure.set_summary(&summary);
                    }
                }
            }));
            let position = Section::form(&ui, "").row(pages::window_position(&mut form));
            pages::align_form(position.ns_view());
            let shared = VStack::new(&ui)
                .spacing(0.0)
                .push(View::new(&ui).content(position, Insets {
                    top: 10.0,
                    left: 12.0,
                    bottom: 10.0,
                    right: 12.0,
                }))
                .push(inset_divider(&ui))
                .push(disclosures[0].clone())
                .push(inset_divider(&ui))
                .push(disclosures[1].clone());
            let root = form.finish(
                SettingsPage::new(&ui, "")
                    .section(GroupBox::with_insets(&ui, editor, cgs::uniform_insets(12.0)))
                    .section(
                        Section::new(&ui, "Shared settings")
                            .description(
                                "Every layout uses these, unless a layout or display sets its own.",
                            )
                            .content(GroupBox::with_insets(&ui, shared, cgs::uniform_insets(0.0))),
                    )
                    .section(HStack::new(&ui).push(Spacer::new(&ui)).push(layout_help(&ui))),
            );
            Self {
                ui,
                model: Rc::downgrade(model),
                root,
                tiles,
                default_control,
                default_popup,
                title,
                summary,
                actions,
                default_items: default_items.into_iter().chain([use_as_default]).collect(),
                restore_item,
                _tile_menus: tile_menus,
                error,
                host,
                floating,
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

    pub fn default_control(&self) -> Rc<dyn NativeView> { self.default_control.clone() }

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
                "{title}’s settings return to their original values. Your default layout, shared settings, and display overrides don’t change."
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
        let mode = match target {
            Target::Layout(mode) => mode,
            Target::Spacing | Target::Displays => {
                let index = (target == Target::Displays) as usize;
                if self.set_shared(target, true) {
                    self.disclosures[index].set_expanded(true);
                }
                return;
            }
        };
        let Some(model) = self.model.upgrade() else {
            return;
        };
        if mode == self.selected.get() && !self.host.ns_view().subviews().is_empty() {
            return self.update_selection(&model.source.borrow());
        }
        // Commit the field being edited, or keep showing it while its value is invalid.
        if !finish_editing(self.host.ns_view()) {
            return self.update_selection(&model.source.borrow());
        }
        self.selected.set(mode);
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
        self.floating.set_hidden(!floating);
        self.restore_item.set_enabled(!floating);
        self.update_selection(&model.source.borrow());
    }

    /// Mount a shared editor before its disclosure expands, or commit its field before
    /// it collapses. Returns false to keep the disclosure as it is.
    fn set_shared(&self, target: Target, expanded: bool) -> bool {
        let index = (target == Target::Displays) as usize;
        let host = &self.shared_hosts[index];
        if !expanded {
            return finish_editing(host.ns_view());
        }
        let Some(model) = self.model.upgrade() else {
            return false;
        };
        let slot = SHARED + index;
        if self.pages.borrow()[slot].is_none() {
            self.pages.borrow_mut()[slot] = Some(match target {
                Target::Spacing => pages::layout_spacing(self.ui, &model),
                _ => editors::display_inspector(self.ui, &model),
            });
        }
        let pages = self.pages.borrow();
        let page = pages[slot].as_ref().unwrap();
        page.synchronize(&model);
        if !host.ns_view().subviews().is_empty() {
            return true;
        }
        host.set_cached_page(page.view.clone());
        // The disclosure expands inside an animation. Lay a newly mounted editor out at
        // its final width first, with animation off, so only the section’s height
        // animates rather than every new control growing from an empty frame.
        objc2_app_kit::NSAnimationContext::beginGrouping();
        let context = objc2_app_kit::NSAnimationContext::currentContext();
        context.setDuration(0.0);
        context.setAllowsImplicitAnimation(false);
        let view = host.ns_view();
        let width = self.disclosures[index].ns_view().frame().size.width - 24.0;
        view.setFrameSize(CGSize::new(width, view.fittingSize().height));
        view.layoutSubtreeIfNeeded();
        objc2_app_kit::NSAnimationContext::endGrouping();
        true
    }

    /// Show the edited and default layouts on the tiles, header, menus and toolbar.
    fn update_selection(&self, source: &ConfigSource) {
        let mode = self.selected.get();
        let default = source.settings.layout.mode;
        if self.shown.replace(Some((mode, default))) == Some((mode, default)) {
            return;
        }
        for (tile, &(_, choice)) in self.tiles.iter().zip(pages::layouts()) {
            tile.set_state(choice == mode, (choice == default).then_some("Default"));
        }
        let title = pages::layout_title(mode);
        self.title.set_text(title);
        self.summary.set_text(LayoutMode::CONFIG_CHOICES[mode.config_choice_index()].1);
        self.actions.accessibility_label(&format!("{title} actions"));
        self.default_popup.set_selected(default.config_choice_index());
        let edited = self.default_items.len() - 1;
        for (index, item) in self.default_items.iter().enumerate() {
            let choice = if index == edited {
                mode
            } else {
                pages::layouts()[index].1
            };
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
                && let Some(page) = &pages[SHARED + index]
            {
                page.synchronize(&model);
            }
        }
        let source = model.source.borrow();
        self.update_selection(&source);
        for (tile, &(_, mode)) in self.tiles.iter().zip(pages::layouts()) {
            let preview = tile.preview();
            preview.set_windows(
                &sample(&source, mode, preview.canvas_size()),
                PreviewAnimation::Immediate,
            );
        }
    }

    /// Connected displays changed: offer the new set without discarding open editors.
    pub fn refresh_displays(&self) {
        let Some(model) = self.model.upgrade() else {
            return;
        };
        self.root.synced_revision.set(None);
        self.root.synchronize(&model);
        if let Some(page) = &self.pages.borrow()[SHARED + 1] {
            page.synced_revision.set(None);
            page.synchronize(&model);
        }
    }
}
impl NativeView for Inspector {
    fn ns_view(&self) -> &objc2_app_kit::NSView { self.root.view.ns_view() }
}

/// A layout’s Use as Default and Restore Defaults… actions, for one layout or, with
/// `None`, whichever layout is being edited.
fn layout_menu(
    ui: &Ui,
    owner: &Weak<Inspector>,
    mode: Option<LayoutMode>,
) -> (Menu, WeakMenuItem, WeakMenuItem) {
    let target = move |inspector: &Inspector| mode.unwrap_or(inspector.selected.get());
    let weak = owner.clone();
    let make_default = MenuItem::new(ui, "Use as Default Layout").on_click(move || {
        if let Some(inspector) = weak.upgrade() {
            inspector.set_default(target(&inspector));
        }
    });
    let weak = owner.clone();
    let restore = MenuItem::new(ui, "Restore Defaults…").on_click(move || {
        if let Some(inspector) = weak.upgrade() {
            inspector.confirm_restore(target(&inspector));
        }
    });
    restore.set_enabled(mode != Some(LayoutMode::Floating));
    let items = (make_default.weak(), restore.weak());
    (
        Menu::new(ui).item(make_default).separator().item(restore),
        items.0,
        items.1,
    )
}

/// Floating has nothing to configure; say what it does instead of showing an empty form.
fn floating_note(ui: &Ui) -> HStack {
    let note = HStack::new(ui)
        .spacing(12.0)
        .alignment(objc2_app_kit::NSLayoutAttribute::CenterY);
    if let Some(symbol) = ImageView::symbol(ui, "macwindow.on.rectangle") {
        let image = symbol.ns_image_view();
        image.setSymbolConfiguration(Some(
            &objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                22.0,
                unsafe { objc2_app_kit::NSFontWeightRegular },
            ),
        ));
        image.setContentTintColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        note.add(symbol);
    }
    note.add(
        VStack::new(ui)
            .spacing(2.0)
            .push(Label::new(ui, "Windows stay where you put them"))
            .push(Caption::new(
                ui,
                "Rift doesn’t arrange windows in floating workspaces, so this layout has no settings of its own.",
            )),
    );
    note
}

/// A row divider aligned with the shared settings’ content.
fn inset_divider(ui: &Ui) -> View {
    View::new(ui).content(Divider::new(ui), Insets {
        top: 0.0,
        left: 12.0,
        bottom: 0.0,
        right: 12.0,
    })
}

/// Collapsed Spacing: the screen-edge and between-window gaps, or their range.
fn spacing_summary(source: &ConfigSource) -> String {
    let gaps = source.settings.layout.gaps.effective_for_display(None);
    let span = |values: &[f64]| {
        let (min, max) =
            values.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        if min == max {
            format!("{min} pt")
        } else {
            format!("{min}–{max} pt")
        }
    };
    let outer = &gaps.outer;
    format!(
        "Edges {} · Windows {}",
        span(&[outer.top, outer.right, outer.bottom, outer.left]),
        span(&[gaps.inner.horizontal, gaps.inner.vertical])
    )
}

/// Collapsed Display overrides: how many displays have their own settings.
fn display_summary(source: &ConfigSource, env: Option<&Env>) -> String {
    let layout = &source.settings.layout;
    let mut custom: Vec<&String> = layout
        .gaps
        .per_display
        .keys()
        .chain(layout.scrolling.per_display.keys())
        .collect();
    custom.sort();
    custom.dedup();
    match custom.as_slice() {
        [] => "None".into(),
        [uuid] => env
            .and_then(|env| editors::display_name(&env.displays.borrow(), uuid))
            .unwrap_or_else(|| "1 display".into()),
        all => format!("{} displays", all.len()),
    }
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

/// Let AppKit commit or validate the active field before its inspector is hidden.
pub(super) fn finish_editing(view: &objc2_app_kit::NSView) -> bool {
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
                .push(WrappingLabel::new(&ui, "Workspaces without their own layout use the Default layout, chosen in the toolbar. Clicking a layout shows its settings without changing the default."))
                .push(WrappingLabel::new(&ui, "Shared settings apply to every layout. A layout can choose its own new-window position, and a display can override spacing and scrolling column widths."))
                .push(Link::new(&ui, "Layout documentation", "https://acsandmann.github.io/rift-docs/layouts/"));
            content.width(320.0);
            HelpPopover::with_content(&ui, content)
        });
        if let Some(anchor) = anchor.load() { help.show(&anchor.into_super().into_super()); }
    })
}
