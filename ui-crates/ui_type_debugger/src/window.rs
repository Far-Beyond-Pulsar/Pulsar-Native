//! Type Debugger Window - Displays type database contents
//! Similar to VS Code's type inspector as a separate window

use gpui::EventEmitter;
use ui_common::drawer_window;

use crate::{TypeDebuggerDrawer, NavigateToType};

drawer_window!(TypeDebuggerWindow, TypeDebuggerDrawer, type_debugger_drawer, "Window.Title.TypeDebugger");

impl EventEmitter<NavigateToType> for TypeDebuggerWindow {}
