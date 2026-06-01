//! Shared tree item rendering logic for hierarchical views
//!
//! This module provides reusable components for rendering tree items with:
//! - Expand/collapse arrows
//! - Drag-and-drop support
//! - Recursive children rendering
//! - Modifier key operations (nest, reorder, un-nest)

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{
    draggable::{DragHandlePosition, Draggable},
    drop_area::DropArea,
    h_flex, v_flex, ActiveTheme, Icon, IconName,
};




