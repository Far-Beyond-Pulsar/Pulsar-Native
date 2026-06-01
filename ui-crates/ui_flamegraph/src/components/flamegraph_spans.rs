//! Span-based flamegraph rendering using divs instead of canvas

use crate::constants::{ROW_HEIGHT, THREAD_LABEL_WIDTH};
use crate::coordinates::time_to_x;
use crate::state::ViewState;
use crate::trace_data::TraceFrame;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use ui::ActiveTheme;


