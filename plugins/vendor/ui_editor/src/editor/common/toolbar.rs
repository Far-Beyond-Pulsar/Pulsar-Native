//! The file/undo controls every physics editor's toolbar starts with.

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::tag::Tag;
use ui::{h_flex, Disableable, IconName, Sizable};

use crate::model::{Asset, Severity};

use super::chrome::{banner, toolbar_divider};
use super::document::{Document, Status};

/// Save, revert, undo and redo for `doc`, as one toolbar group. `id` keeps
/// the button ids unique when two editors are open.
pub fn document_controls<A: Asset>(
    id: &'static str,
    doc: &Entity<Document<A>>,
    cx: &App,
) -> impl IntoElement {
    let d = doc.read(cx);
    let (dirty, can_save, can_undo, can_redo) = (d.is_dirty(), d.can_save(), d.can_undo(), d.can_redo());
    let undo_tip = d
        .undo_label()
        .filter(|l| !l.is_empty())
        .map_or("Undo (Ctrl+Z)".to_string(), |l| format!("Undo {l} (Ctrl+Z)"));
    let redo_tip = d
        .redo_label()
        .filter(|l| !l.is_empty())
        .map_or("Redo (Ctrl+Shift+Z)".to_string(), |l| format!("Redo {l} (Ctrl+Shift+Z)"));

    let (save_doc, revert_doc, undo_doc, redo_doc) = (doc.clone(), doc.clone(), doc.clone(), doc.clone());

    h_flex()
        .gap_1()
        .items_center()
        .child(
            Button::new(SharedString::from(format!("{id}-save")))
                .icon(IconName::FloppyDisk)
                .label("Save")
                .small()
                .when(dirty, |b| b.primary())
                .when(!dirty, |b| b.ghost())
                .disabled(!can_save)
                .tooltip("Save (Ctrl+S)")
                .on_click(move |_, _, cx| save_doc.update(cx, |d, cx| d.save(cx))),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-revert")))
                .icon(IconName::Refresh)
                .ghost()
                .small()
                .disabled(!dirty || !can_save)
                .tooltip("Discard unsaved changes and reload the file")
                .on_click(move |_, _, cx| revert_doc.update(cx, |d, cx| d.revert(cx))),
        )
        .child(toolbar_divider(cx))
        .child(
            Button::new(SharedString::from(format!("{id}-undo")))
                .icon(IconName::Undo)
                .ghost()
                .small()
                .disabled(!can_undo)
                .tooltip(undo_tip)
                .on_click(move |_, _, cx| undo_doc.update(cx, |d, cx| d.undo(cx))),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-redo")))
                .icon(IconName::Redo)
                .ghost()
                .small()
                .disabled(!can_redo)
                .tooltip(redo_tip)
                .on_click(move |_, _, cx| redo_doc.update(cx, |d, cx| d.redo(cx))),
        )
}

/// A tag saying where the document stands: unsaved, saved, or a save failed.
pub fn status_tag<A: Asset>(doc: &Entity<Document<A>>, cx: &App) -> AnyElement {
    let d = doc.read(cx);
    if d.unreadable().is_some() {
        return Tag::danger().small().child("Unreadable").into_any_element();
    }
    match d.status() {
        Some(Status::SaveFailed(_)) => Tag::danger().small().child("Save failed").into_any_element(),
        _ if d.is_dirty() => Tag::warning().small().child("Unsaved changes").into_any_element(),
        _ if d.path().is_none() => Tag::secondary().small().child("Not on disk").into_any_element(),
        Some(Status::Reverted) => Tag::info().small().child("Reloaded").into_any_element(),
        _ => Tag::success().small().child("Saved").into_any_element(),
    }
}

/// Banners for problems with the file itself: unreadable, failed save, or
/// repairs made while loading. Empty when the file is fine.
pub fn file_banners<A: Asset>(id: &'static str, doc: &Entity<Document<A>>, cx: &App) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let d = doc.read(cx);

    if let Some(error) = d.unreadable() {
        let fresh = doc.clone();
        out.push(
            banner(
                Severity::Error,
                format!("This file couldn't be read ({error}). It won't be overwritten unless you start over."),
                vec![Button::new(SharedString::from(format!("{id}-start-over")))
                    .label("Start over")
                    .small()
                    .on_click(move |_, _, cx| fresh.update(cx, |d, cx| d.start_fresh(cx)))],
                cx,
            )
            .into_any_element(),
        );
    }
    if let Some(Status::SaveFailed(error)) = d.status() {
        out.push(banner(Severity::Error, format!("Couldn't save: {error}"), Vec::new(), cx).into_any_element());
    }
    if !d.load_notes().is_empty() {
        let dismiss = doc.clone();
        out.push(
            banner(
                Severity::Warning,
                format!("Repaired while loading: {}", d.load_notes().join("; ")),
                vec![Button::new(SharedString::from(format!("{id}-dismiss-notes")))
                    .label("Dismiss")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| dismiss.update(cx, |d, cx| d.dismiss_load_notes(cx)))],
                cx,
            )
            .into_any_element(),
        );
    }
    out
}
