use engine_backend::{services::gpu_renderer::GpuRendererBuilder, GizmoMode};

#[test]
fn gizmo_mailbox_does_not_require_a_scene_store_handle() {
    let renderer = GpuRendererBuilder::new(1, 1).build();
    let mailbox = renderer.editor_mailbox().expect("renderer mailbox");

    // Gizmo mode is editor/renderer state. Queueing it must remain usable
    // without retaining or mutating the SceneDB store from the mailbox.
    mailbox.queue_gizmo(GizmoMode::Rotate);
    mailbox.queue_deselect();
}
