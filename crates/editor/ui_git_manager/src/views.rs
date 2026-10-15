//! Git Manager UI views

mod branches;
mod changes;
mod chrome;
mod commit_detail;
mod commit_list;
mod common;
mod desktop;
mod diff;
mod tree_layout;

pub(crate) use branches::render_branch_menu;
pub(crate) use chrome::{
    render_banners, render_failed, render_loading, render_not_a_repository, render_top_bar,
};
pub(crate) use desktop::render_desktop_layout;
pub(crate) use tree_layout::render_tree_layout;
