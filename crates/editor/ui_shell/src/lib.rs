//! The shell pieces shared by the editor and the Hub: opening a window
//! through the `WindowManager` ([`PulsarWindowExt`]), the account menu and
//! theme picker, and the registrations an editor tool uses to add itself to
//! the shell ([`editor_tools`]).
//!
//! Kept apart from `ui_common` (the editor's grab-bag, which links the plugin
//! API, the script VM and the renderer's component types) so a host that only
//! needs these, such as the Hub, links none of that.

pub mod editor_tools;
pub mod open_window;
pub mod profile_dropdown;
pub mod theme_dropdown;

pub use open_window::PulsarWindowExt;
pub use profile_dropdown::{AccountMenuItem, ProfileDropdown, ProfileDropdownEvent};
pub use theme_dropdown::ThemePicker;
