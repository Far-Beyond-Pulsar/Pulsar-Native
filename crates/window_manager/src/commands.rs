use gpui::{AnyView, App, Pixels, Point, Size, Window, WindowOptions};
use ui_types_common::window_types::{WindowId, WindowRequest};

#[derive(Debug)]
pub enum WindowCommand {
    Create(CreateWindowCommand),
    Close(CloseWindowCommand),
    Focus(FocusWindowCommand),
    Minimize(MinimizeWindowCommand),
    Maximize(MaximizeWindowCommand),
    Move(MoveWindowCommand),
    Resize(ResizeWindowCommand),
    UpdateTitle(UpdateTitleCommand),
}

pub struct CreateWindowCommand {
    pub window_type: WindowRequest,
    pub options: WindowOptions,
    pub content_builder: Box<dyn FnOnce(&mut Window, &mut App) -> AnyView + Send>,
    pub parent_window: Option<WindowId>,
}

impl std::fmt::Debug for CreateWindowCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateWindowCommand")
            .field("window_type", &self.window_type)
            .field("options", &"WindowOptions { ... }")
            .field("content_builder", &"<closure>")
            .field("parent_window", &self.parent_window)
            .finish()
    }
}

impl CreateWindowCommand {
    pub fn new<F>(
        window_type: WindowRequest,
        options: WindowOptions,
        content_builder: F,
    ) -> Self
    where
        F: FnOnce(&mut Window, &mut App) -> AnyView + Send + 'static,
    {
        Self {
            window_type,
            options,
            content_builder: Box::new(content_builder),
            parent_window: None,
        }
    }

    pub fn with_parent(mut self, parent_window: WindowId) -> Self {
        self.parent_window = Some(parent_window);
        self
    }
}

#[derive(Debug, Clone)]
pub struct CloseWindowCommand {
    pub window_id: WindowId,
    pub force: bool,
}

impl CloseWindowCommand {
    pub fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            force: false,
        }
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct FocusWindowCommand {
    pub window_id: WindowId,
}

#[derive(Debug, Clone)]
pub struct MinimizeWindowCommand {
    pub window_id: WindowId,
}

#[derive(Debug, Clone)]
pub struct MaximizeWindowCommand {
    pub window_id: WindowId,
    pub restore: bool,
}

#[derive(Debug, Clone)]
pub struct MoveWindowCommand {
    pub window_id: WindowId,
    pub position: Point<Pixels>,
}

#[derive(Debug, Clone)]
pub struct ResizeWindowCommand {
    pub window_id: WindowId,
    pub size: Size<Pixels>,
}

#[derive(Debug, Clone)]
pub struct UpdateTitleCommand {
    pub window_id: WindowId,
    pub title: String,
}

pub enum WindowCommandResult {
    Created { window_id: WindowId },
    Closed { window_id: WindowId },
    Focused { window_id: WindowId },
    Minimized { window_id: WindowId },
    Maximized { window_id: WindowId },
    Moved { window_id: WindowId },
    Resized { window_id: WindowId },
    TitleUpdated { window_id: WindowId },
}

impl WindowCommandResult {
    pub fn window_id(&self) -> WindowId {
        match self {
            Self::Created { window_id } => *window_id,
            Self::Closed { window_id } => *window_id,
            Self::Focused { window_id } => *window_id,
            Self::Minimized { window_id } => *window_id,
            Self::Maximized { window_id } => *window_id,
            Self::Moved { window_id } => *window_id,
            Self::Resized { window_id } => *window_id,
            Self::TitleUpdated { window_id } => *window_id,
        }
    }
}