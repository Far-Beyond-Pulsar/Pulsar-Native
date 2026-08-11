// Helper canvas for height measurement
fn height_canvas(panel: &gpui::Entity<AgentChatPanel>, ix: usize) -> impl gpui::IntoElement {
    let p = panel.clone();
    gpui::canvas(
        move |bounds, _, cx| {
            p.update(cx, |panel, cx| {
                let measured = bounds.size.height;
                if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                    panel.display_item_heights.insert(ix, measured);
                    cx.notify();
                }
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}
