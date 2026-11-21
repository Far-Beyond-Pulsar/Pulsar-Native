//! The "Open take" file picker shared by the panels.

use crate::model::SessionModel;
use gpui::{Context, Entity, PathPromptOptions};

/// Asks for a `.mocap` file and opens it in `model`.
pub fn pick_take<T: 'static>(model: &Entity<SessionModel>, cx: &mut Context<T>) {
    let model = model.clone();
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Open a .mocap take".into()),
    });
    cx.spawn(async move |_this, cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        let _ = cx.update(|cx| {
            model.update(cx, |m, cx| {
                m.open_take(path);
                cx.notify();
            })
        });
    })
    .detach();
}

/// Asks for an ONNX keypoint model and uses it for body tracking.
pub fn pick_pose_model<T: 'static>(model: &Entity<SessionModel>, cx: &mut Context<T>) {
    let model = model.clone();
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Choose a single-person keypoint model (.onnx)".into()),
    });
    cx.spawn(async move |_this, cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        let _ = cx.update(|cx| {
            model.update(cx, |m, cx| {
                m.set_pose_model(path);
                cx.notify();
            })
        });
    })
    .detach();
}
