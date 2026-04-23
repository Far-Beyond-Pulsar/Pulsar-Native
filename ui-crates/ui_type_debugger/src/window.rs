//! Type Debugger Window - Displays type database contents
//! Similar to VS Code's type inspector as a separate window

use gpui::EventEmitter;
use ui_common::pulsar_drawer_window;

use crate::{NavigateToType, TypeDebuggerDrawer};

pulsar_drawer_window!(
    TypeDebuggerWindow,
    TypeDebuggerDrawer,
    type_debugger_drawer,
    "Window.Title.TypeDebugger",
    1000.0,
    700.0
);

impl EventEmitter<NavigateToType> for TypeDebuggerWindow {}
