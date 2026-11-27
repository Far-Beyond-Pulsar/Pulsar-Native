//! The window itself, headless, over a published scene: it draws every view
//! and tab, follows the scene while Live, holds still while paused, and
//! reads only the pages it shows.

use std::sync::Arc;

use engine_backend::scene::{new_scene, SharedScene};
use engine_backend::services::scene_directory;
use gpui::{px, size, Entity as View, TestAppContext, VisualTestContext};
use pulsar_scene_model::attachments::{self, NewInstance};
use pulsar_scene_model::{Name, Parent, StableId, Transform, UnresolvedComponent};
use pulsar_scenedb::{Entity, World};

use crate::screen::{ByteScope, DataTab, SceneInspectorView};
use crate::utils::detail::ComponentRef;
use crate::utils::outline::Mode;

/// Objects in the scene: enough that a column of a `Transform` spans pages.
const OBJECTS: usize = 40_000;

/// A component nothing describes.
struct Opaque(#[allow(dead_code)] [u8; 40]);

fn populate(world: &mut World) -> Vec<Entity> {
    let mut objects = Vec::with_capacity(OBJECTS);
    for index in 0..OBJECTS {
        let entity = world.spawn();
        world.insert(entity, StableId(format!("object-{index}")));
        world.insert(entity, Name(format!("Object {index}")));
        world.insert(
            entity,
            Transform {
                position: [index as f64, 0.0, 0.0],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
        );
        if index % 10 != 0 {
            world.insert(entity, Parent(objects[index - index % 10]));
        }
        objects.push(entity);
    }
    world.insert(objects[0], Opaque([7; 40]));
    let instance =
        attachments::spawn_instance(world, objects[0], NewInstance::new("Mystery")).unwrap();
    world.insert(
        instance,
        UnresolvedComponent {
            data: serde_json::json!({ "samples": (0..5_000).collect::<Vec<u32>>() }),
            reason: "unknown class".into(),
        },
    );
    objects
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.run_until_parked();
}

fn act(
    view: &View<SceneInspectorView>,
    cx: &mut VisualTestContext,
    action: impl FnOnce(&mut SceneInspectorView, &mut gpui::Context<SceneInspectorView>),
) {
    view.update(cx, action);
    draw(cx);
}

fn position(view: &SceneInspectorView) -> Option<serde_json::Value> {
    let fields = view.data.as_ref()?.fields.as_ref()?;
    fields.value_at("0/1").cloned()
}

#[gpui::test]
fn every_view_and_tab_draws_over_a_live_scene(cx: &mut TestAppContext) {
    let scene: SharedScene = Arc::new(parking_lot::RwLock::new(new_scene()));
    let objects = populate(&mut scene.write().world);
    let published = scene_directory::publish(&scene, Box::new(|| "Test scene".to_string()));

    cx.update(|cx| ui::init(cx));
    let window = cx.open_window(size(px(1600.), px(940.)), SceneInspectorView::new);
    let view = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let cx = &mut cx;
    draw(cx);
    act(&view, cx, |view, cx| view.tick(cx));

    view.read_with(cx, |view, _| {
        let index = view.index.as_ref().expect("the scene was scanned");
        assert_eq!(index.object_count, OBJECTS);
        assert_eq!(view.rows.len(), OBJECTS, "the tree starts open");
    });

    // An object: its Transform is shown first, as fields.
    let first = objects[0];
    act(&view, cx, |view, cx| view.select(first, None, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.data.as_ref().unwrap().name, "Transform");
        assert!(view.field_rows.iter().any(|row| row.label == "position"));
    });

    for mode in Mode::ALL {
        act(&view, cx, |view, cx| view.set_mode(mode, cx));
    }
    // Open the first type and the first archetype.
    for mode in [Mode::Components, Mode::Archetypes] {
        act(&view, cx, |view, cx| {
            view.set_mode(mode, cx);
            view.toggle_row(0, cx);
        });
    }
    act(&view, cx, |view, cx| view.set_mode(Mode::Objects, cx));

    // A live change reaches the view and is marked.
    scene.write().world.get_mut::<Transform>(first).unwrap().position[1] = 42.0;
    act(&view, cx, |view, cx| view.request_refresh(false, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(position(view), Some(serde_json::json!(42.0)));
        // `position` is one row.
        assert!(view.changed_fields.contains_key("0"));
        assert!(!view.changed_bytes.is_empty());
    });

    // Paused, a change waits for Refresh.
    act(&view, cx, |view, _| view.live = false);
    scene.write().world.get_mut::<Transform>(first).unwrap().position[1] = 7.0;
    act(&view, cx, |view, cx| {
        view.refresh.value_due = std::time::Instant::now();
        view.tick(cx);
    });
    view.read_with(cx, |view, _| {
        assert_eq!(position(view), Some(serde_json::json!(42.0)));
    });
    act(&view, cx, |view, cx| view.request_refresh(true, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(position(view), Some(serde_json::json!(7.0)));
    });
    act(&view, cx, |view, _| view.live = true);

    // Bytes, of this entity and then of every entity's Transform.
    for tab in [DataTab::Hex, DataTab::Binary] {
        act(&view, cx, |view, cx| view.set_tab(tab, cx));
    }
    act(&view, cx, |view, cx| view.set_tab(DataTab::Hex, cx));
    act(&view, cx, |view, cx| view.set_scope(ByteScope::Column, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.bytes.column.len(), OBJECTS);
        assert!(view.page_count() > 1);
        let page = view.bytes.page_data.as_ref().expect("a page was read");
        assert!(page.request.entities.len() < OBJECTS, "only a page is read");
        assert!(view.byte(view.bytes.cursor).is_some());
    });
    // Jumping to the last record turns to the last page and reads it.
    let stride = std::mem::size_of::<Transform>();
    let last = format!("{:#x}", stride * (OBJECTS - 1));
    act(&view, cx, |view, cx| view.go_to_offset(&last, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.bytes.page, view.page_count() - 1);
        assert!(view.byte(view.bytes.cursor).is_some());
    });
    for per_row in [8, 32] {
        act(&view, cx, |view, cx| view.set_per_row(per_row, cx));
        view.read_with(cx, |view, _| {
            assert!(view.byte(view.bytes.cursor).is_some());
        });
    }

    // The grid's keyboard moves the cursor and copies.
    act(&view, cx, |view, cx| view.set_scope(ByteScope::Value, cx));
    act(&view, cx, |view, cx| {
        view.set_per_row(16, cx);
        view.place_cursor(0, false, cx);
    });
    cx.update(|window, cx| {
        let focus = view.read(cx).bytes_focus.clone();
        window.focus(&focus, cx);
    });
    draw(cx);
    cx.simulate_keystrokes("down right shift-right shift-right");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.bytes.selection(), (17, 19));
    });
    cx.simulate_keystrokes("ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    assert_eq!(copied.split(' ').count(), 3);

    // A type nothing describes: the Fields tab offers its bytes instead.
    let opaque = ComponentRef {
        entity: first,
        id: pulsar_scenedb::component_id::<Opaque>(),
    };
    act(&view, cx, |view, cx| {
        view.set_tab(DataTab::Fields, cx);
        view.select_component(opaque, cx);
    });
    view.read_with(cx, |view, _| {
        let data = view.data.as_ref().unwrap();
        assert!(data.fields.is_none());
        assert_eq!(data.bytes.len(), 40);
    });
    act(&view, cx, |view, cx| view.set_tab(DataTab::Binary, cx));

    // The attached instance: a payload with a long list, kept to a few rows.
    let instance = attachments::instances(&scene.read().world, first)[0];
    act(&view, cx, |view, cx| {
        view.set_tab(DataTab::Fields, cx);
        view.follow_link(instance, cx);
    });
    act(&view, cx, |view, cx| {
        let target = ComponentRef {
            entity: instance,
            id: pulsar_scenedb::component_id::<UnresolvedComponent>(),
        };
        view.select_component(target, cx);
    });
    act(&view, cx, |view, _| {
        // `data` is open by default; open its list.
        view.toggled.toggle("1/samples");
        view.rebuild_field_rows();
    });
    view.read_with(cx, |view, _| {
        // 5,000 items show as 50 ranges.
        let rows = view.field_rows.len();
        assert!((50..60).contains(&rows), "{rows}");
    });

    // History goes back to the object, and a despawn is said so.
    act(&view, cx, |view, cx| view.go_back(cx));
    view.read_with(cx, |view, _| assert_eq!(view.selected, Some(first)));
    scene.write().world.despawn(first);
    act(&view, cx, |view, cx| view.request_refresh(true, cx));
    view.read_with(cx, |view, _| {
        assert!(view.detail.is_none() && view.data.is_none());
    });

    // The scene closing leaves an empty window.
    drop(published);
    drop(scene);
    act(&view, cx, |view, cx| view.refresh_scenes(cx));
    view.read_with(cx, |view, _| {
        assert!(view.scene.is_none() && view.index.is_none());
    });
}
