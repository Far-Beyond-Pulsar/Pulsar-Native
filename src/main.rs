use gpui::{
    div, prelude::*, px, rgb, size, App, Application, 
    Bounds, Context, SharedString, TitlebarOptions, 
    Window, WindowBounds, WindowOptions
};
use std::sync::Arc;
use std::{sync::RwLock, thread};
use std::time::Duration;
use rand::Rng;
use chrono::Local;

mod components;
use components::{top_bar, menu_bar, tab_bar, main_content, status_bar, Tab};

const APP_TITLE: &str = "PULSAR ENGINE";


struct GameEngine {
    branch: Arc<RwLock<SharedString>>,
    memory: Arc<RwLock<SharedString>>,
    title:  Arc<RwLock<SharedString>>,
    time:   Arc<RwLock<SharedString>>,
    fps:    Arc<RwLock<SharedString>>,
}

impl GameEngine {
    fn new() -> Self {
        GameEngine {
            title: Arc::new(RwLock::new("PULSAR ENGINE".into())),
            branch: Arc::new(RwLock::new("feature/physics-update".into())),
            fps: Arc::new(RwLock::new("3001".into())),
            memory: Arc::new(RwLock::new("548".into())),
            time: Arc::new(RwLock::new("11:40:19 PM".into())),
        }
    }

    fn spawn_status_updater(&self) {
        let fps = Arc::clone(&self.fps);
        let memory = Arc::clone(&self.memory);
        let time = Arc::clone(&self.time);

        thread::spawn(move || {
            let mut rng = rand::thread_rng();
            loop {
                thread::sleep(Duration::from_millis(1000));
                
                if let Ok(mut fps_write) = fps.write() {
                    *fps_write = format!("{}", rng.gen_range(2000..4000)).into();
                }
                
                if let Ok(mut memory_write) = memory.write() {
                    *memory_write = format!("{}", rng.gen_range(100..1000)).into();
                }
                
                if let Ok(mut time_write) = time.write() {
                    *time_write = Local::now().format("%I:%M:%S %p").to_string().into();
                }
            }
        });
    }
}

impl Render for GameEngine {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x000000))
            .child(top_bar(APP_TITLE.into()))
            .child(menu_bar())
            .child(tab_bar(0, &vec!["Tab 1", "Tab 2", "Tab 3"]))
            .child(main_content(&Tab::LevelEditor))
            .child(status_bar(
                self.fps, 
                self.memory, 
                self.time, 
                self.branch)
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                let engine = cx.new(|_| GameEngine {
                    title: Arc::new(RwLock::new("PULSAR ENGINE".into())),
                    branch: Arc::new(RwLock::new("feature/physics-update".into())),
                    fps: Arc::new(RwLock::new("3001".into())),
                    memory: Arc::new(RwLock::new("548".into())),
                    time: Arc::new(RwLock::new("11:40:19 PM".into())),
                });
                
                engine
            },
        )
        .unwrap();
    });
}