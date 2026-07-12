use regex::{Regex, RegexBuilder};
use tracing::warn;

use crate::common::config::{AppWorkspaceRule, WorkspaceSelector};

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowRuleContext<'a> {
    pub app_bundle_id: Option<&'a str>,
    pub app_name: Option<&'a str>,
    pub window_title: Option<&'a str>,
    pub ax_role: Option<&'a str>,
    pub ax_subrole: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRuleDecision {
    NoMatch,
    Unmanaged,
    Managed {
        workspace: Option<WorkspaceSelector>,
        floating: bool,
    },
}

#[derive(Debug, Clone)]
struct CompiledRule {
    rule: AppWorkspaceRule,
    title_regex: Option<Regex>,
}

/// Compiles and evaluates app policy without depending on workspaces, windows,
/// the reactor, or the layout engine.
#[derive(Debug, Clone, Default)]
pub struct AppRuleEngine {
    rules: Vec<CompiledRule>,
}

impl AppRuleEngine {
    pub fn new(rules: &[AppWorkspaceRule]) -> Self {
        let rules = rules
            .iter()
            .cloned()
            .map(|rule| {
                let title_regex =
                    rule.title_regex.as_deref().filter(|value| !value.is_empty()).and_then(
                        |value| {
                            RegexBuilder::new(value)
                            .case_insensitive(true)
                            .build()
                            .map_err(|error| {
                                warn!(%error, pattern = value, "invalid title regex in app rule");
                            })
                            .ok()
                        },
                    );
                CompiledRule { rule, title_regex }
            })
            .collect();
        Self { rules }
    }

    pub fn evaluate(&self, context: WindowRuleContext<'_>) -> AppRuleDecision {
        let best = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.matches(context))
            .max_by_key(|(index, rule)| (rule.specificity(), std::cmp::Reverse(*index)));
        let Some((_, matched)) = best else {
            return AppRuleDecision::NoMatch;
        };
        if !matched.rule.manage {
            AppRuleDecision::Unmanaged
        } else {
            AppRuleDecision::Managed {
                workspace: matched.rule.workspace.clone(),
                floating: matched.rule.floating,
            }
        }
    }
}

impl CompiledRule {
    fn matches(&self, context: WindowRuleContext<'_>) -> bool {
        optional_eq_ignore_case(self.rule.app_id.as_deref(), context.app_bundle_id)
            && optional_fuzzy_name(self.rule.app_name.as_deref(), context.app_name)
            && optional_regex(
                self.rule.title_regex.as_deref(),
                self.title_regex.as_ref(),
                context.window_title,
            )
            && optional_contains(self.rule.title_substring.as_deref(), context.window_title)
            && optional_exact(self.rule.ax_role.as_deref(), context.ax_role)
            && optional_exact(self.rule.ax_subrole.as_deref(), context.ax_subrole)
    }

    fn specificity(&self) -> usize {
        [
            self.rule.app_id.as_deref(),
            self.rule.app_name.as_deref(),
            self.rule.title_regex.as_deref(),
            self.rule.title_substring.as_deref(),
            self.rule.ax_role.as_deref(),
            self.rule.ax_subrole.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .count()
    }
}

fn optional_eq_ignore_case(rule: Option<&str>, actual: Option<&str>) -> bool {
    rule.is_none_or(|rule| actual.is_some_and(|actual| rule.eq_ignore_ascii_case(actual)))
}
fn optional_fuzzy_name(rule: Option<&str>, actual: Option<&str>) -> bool {
    rule.is_none_or(|rule| {
        actual.is_some_and(|actual| {
            let (rule, actual) = (rule.to_lowercase(), actual.to_lowercase());
            rule.contains(&actual) || actual.contains(&rule)
        })
    })
}
fn optional_regex(pattern: Option<&str>, regex: Option<&Regex>, actual: Option<&str>) -> bool {
    pattern.is_none_or(|pattern| {
        !pattern.is_empty()
            && regex.is_some_and(|regex| actual.is_some_and(|actual| regex.is_match(actual)))
    })
}
fn optional_contains(rule: Option<&str>, actual: Option<&str>) -> bool {
    rule.is_none_or(|rule| {
        !rule.is_empty()
            && actual.is_some_and(|actual| actual.to_lowercase().contains(&rule.to_lowercase()))
    })
}
fn optional_exact(rule: Option<&str>, actual: Option<&str>) -> bool {
    rule.is_none_or(|rule| !rule.is_empty() && actual == Some(rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_without_workspace_or_layout_state() {
        let rule = AppWorkspaceRule {
            app_id: Some("com.example.Editor".into()),
            workspace: None,
            floating: true,
            manage: true,
            app_name: None,
            title_regex: Some("project \\d+".into()),
            title_substring: None,
            ax_role: None,
            ax_subrole: None,
        };
        let engine = AppRuleEngine::new(&[rule]);
        assert_eq!(
            engine.evaluate(WindowRuleContext {
                app_bundle_id: Some("COM.EXAMPLE.EDITOR"),
                window_title: Some("Project 42"),
                ..Default::default()
            }),
            AppRuleDecision::Managed {
                workspace: None,
                floating: true
            }
        );
    }
}
