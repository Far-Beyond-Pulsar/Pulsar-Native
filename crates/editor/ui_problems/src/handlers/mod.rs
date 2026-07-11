use gpui::*;

use crate::components::ProblemsDrawer;
use crate::utils::actions::*;
use crate::utils::types::DiagnosticSeverity;

pub fn on_filter_all(
    d: &mut ProblemsDrawer,
    _: &FilterAll,
    _: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) {
    d.set_filter(None, cx);
}

pub fn on_filter_errors(
    d: &mut ProblemsDrawer,
    _: &FilterErrors,
    _: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) {
    d.set_filter(Some(DiagnosticSeverity::Error), cx);
}

pub fn on_filter_warnings(
    d: &mut ProblemsDrawer,
    _: &FilterWarnings,
    _: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) {
    d.set_filter(Some(DiagnosticSeverity::Warning), cx);
}

pub fn on_filter_info(
    d: &mut ProblemsDrawer,
    _: &FilterInfo,
    _: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) {
    d.set_filter(Some(DiagnosticSeverity::Information), cx);
}
