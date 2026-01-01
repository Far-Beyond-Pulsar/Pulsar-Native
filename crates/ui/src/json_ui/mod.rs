use gpui::*;
use std::path::{Path, PathBuf};

pub mod schema;
pub mod parser;
pub mod renderer;
pub mod hot_reload;

pub use schema::*;
pub use parser::*;
pub use renderer::*;
pub use hot_reload::*;

pub struct JsonCanvas {
    root_path: PathBuf,
    current_ui: Option<UiComponent>,
    hot_reload_manager: HotReloadManager,
    parser: UiParser,
}

impl JsonCanvas {
    pub fn new(root_path: impl AsRef<Path>) -> Self {
        let root_path = root_path.as_ref().to_path_buf();
        let base_path = root_path.parent().unwrap_or(Path::new("."));

        Self {
            root_path: root_path.clone(),
            current_ui: None,
            hot_reload_manager: HotReloadManager::new(),
            parser: UiParser::new(base_path),
        }
    }

    pub fn load(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let ui = self.parser.parse_document(&self.root_path)?;
        self.current_ui = Some(ui);
        Ok(())
    }

    pub fn start_hot_reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.hot_reload_manager.start_watching(&self.root_path)?;
        Ok(())
    }

    pub fn check_and_reload(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let changes = self.hot_reload_manager.check_for_changes();

        if !changes.is_empty() {
            self.parser.clear_cache();
            self.load()?;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.parser.clear_cache();
        self.load()
    }

    pub fn is_loaded(&self) -> bool {
        self.current_ui.is_some()
    }

    pub fn get_ui(&self) -> Option<&UiComponent> {
        self.current_ui.as_ref()
    }

    pub fn load_from_string(&mut self, json_content: &str) -> Result<(), Box<dyn std::error::Error>> {
        let ui = self.parser.parse_from_string(json_content)?;
        self.current_ui = Some(ui);
        Ok(())
    }
}

impl Render for JsonCanvas {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Ok(reloaded) = self.check_and_reload() {
            if reloaded {
                cx.notify();
            }
        }

        if let Some(ref ui) = self.current_ui {
            UiRenderer::render_component(ui, cx)
        } else {
            div()
                .p_4()
                .child("Loading JSON UI...")
                .into_any_element()
        }
    }
}

pub fn create_json_canvas_view(root_path: impl AsRef<Path>, cx: &mut Context<impl Render>) -> Entity<JsonCanvas> {
    cx.new(|_cx| {
        let mut canvas = JsonCanvas::new(root_path);
        if let Err(e) = canvas.load() {
            tracing::error!("Error loading JSON UI: {}", e);
        }
        if let Err(e) = canvas.start_hot_reload() {
            tracing::error!("Error starting hot reload: {}", e);
        }
        canvas
    })
}