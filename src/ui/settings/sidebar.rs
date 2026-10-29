//! Settings destinations and navigation behavior.
use cgs::Ui;
use cgs::settings::{SettingsBadge, SettingsDestination, SettingsNavigation};
use cgs::view::NativeView;
use objc2_app_kit::NSView;

use super::{CATEGORIES, Category};

pub(super) struct SettingsSidebar {
    navigation: SettingsNavigation<Category>,
}

impl SettingsSidebar {
    pub(super) fn new(ui: &Ui, select: impl FnMut(Category) + 'static) -> Self {
        let destinations = CATEGORIES
            .into_iter()
            .map(|(id, title, symbol)| SettingsDestination {
                id,
                title,
                symbol,
                badge: match id {
                    Category::General | Category::Advanced | Category::About => SettingsBadge::Gray,
                    Category::Layouts => SettingsBadge::Blue,
                    Category::Workspaces => SettingsBadge::Violet,
                    Category::Rules => SettingsBadge::Brown,
                    Category::Keyboard | Category::Mouse => SettingsBadge::Slate,
                    Category::MenuBar | Category::Overview | Category::StackTabs => {
                        SettingsBadge::Teal
                    }
                },
                starts_group: matches!(
                    id,
                    Category::Keyboard | Category::MenuBar | Category::Advanced | Category::About
                ),
            })
            .collect();
        let navigation = SettingsNavigation::new(ui, destinations, select);
        navigation.identifier("rift.settings.navigation");
        navigation.accessibility_label("Settings categories");
        Self { navigation }
    }

    pub(super) fn set_selected(&self, category: Category) {
        self.navigation.set_selected(&category);
    }
}

impl NativeView for SettingsSidebar {
    fn ns_view(&self) -> &NSView { self.navigation.ns_view() }
}
