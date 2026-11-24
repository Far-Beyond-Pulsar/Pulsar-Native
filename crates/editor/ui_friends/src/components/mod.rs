//! Render functions of the friends window: header, tab bar, friend list, add-friend bar and
//! popover.

pub mod add_friend;
pub mod friend_list;
pub mod header;
pub mod popover;
pub mod tab_bar;

pub use add_friend::render_add_friend_bar;
pub use friend_list::*;
pub use header::render_header;
pub use tab_bar::*;
