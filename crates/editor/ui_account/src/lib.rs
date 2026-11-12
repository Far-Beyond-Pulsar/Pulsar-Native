//! The account and theme widgets shared by the editor and the Hub.
//!
//! Kept apart from `ui_common` (the editor's grab-bag, which links the plugin
//! API, the script VM and the renderer's component types) so a host that only
//! shows a profile menu and a theme picker, such as the Hub, links neither.

pub mod profile_dropdown;
pub mod theme_dropdown;

pub use profile_dropdown::{AccountMenuItem, ProfileDropdown, ProfileDropdownEvent};
pub use theme_dropdown::ThemePicker;
