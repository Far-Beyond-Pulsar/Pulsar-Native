//! Terminal core using Alacritty's Term

use alacritty_terminal::{
    Term,
    event::{Event as AlacTermEvent, EventListener, Notify, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    sync::FairMutex,
    term::{Config, TermMode, RenderableCursor, cell::Cell},
    tty,
};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::*;
use std::sync::Arc;
use std::path::PathBuf;
use std::collections::VecDeque;

// Terminal key context (like Input's CONTEXT)
pub const TERMINAL_CONTEXT: &str = "Terminal";

// Terminal actions - Tab must be handled by action to prevent focus navigation
actions!(
    terminal,
    [
        SendTab,
        SendShiftTab,
    ]
);

/// Initialize terminal keybindings
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, Some(TERMINAL_CONTEXT)),
        KeyBinding::new("shift-tab", SendShiftTab, Some(TERMINAL_CONTEXT)),
    ]);
}

/// Indexed cell (from Zed)
#[derive(Debug, Clone)]
pub struct IndexedCell {
    pub point: AlacPoint,
    pub cell: Cell,
}

impl std::ops::Deref for IndexedCell {
    type Target = Cell;

    fn deref(&self) -> &Self::Target {
        &self.cell
    }
}

/// Terminal content cache (from Zed)
#[derive(Clone)]
pub struct TerminalContent {
    pub cells: Vec<IndexedCell>,
    pub mode: TermMode,
    pub display_offset: usize,
    pub cursor: RenderableCursor,
    pub cursor_char: char,
    pub terminal_bounds: TerminalBounds,
}

impl Default for TerminalContent {
    fn default() -> Self {
        TerminalContent {
            cells: Default::default(),
            mode: Default::default(),
            display_offset: Default::default(),
            cursor: RenderableCursor {
                shape: alacritty_terminal::vte::ansi::CursorShape::Block,
                point: AlacPoint::new(Line(0), Column(0)),
            },
            cursor_char: Default::default(),
            terminal_bounds: Default::default(),
        }
    }
}

/// Events that flow upward from the terminal
#[derive(Clone, Debug)]
pub enum Event {
    TitleChanged,
    CloseTerminal,
    Bell,
    Wakeup,
}

/// Terminal bounds for rendering
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBounds {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub bounds: Bounds<Pixels>,
}

impl TerminalBounds {
    pub fn new(line_height: Pixels, cell_width: Pixels, bounds: Bounds<Pixels>) -> Self {
        TerminalBounds {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn num_lines(&self) -> usize {
        (self.bounds.size.height / self.line_height).floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        (self.bounds.size.width / self.cell_width).floor() as usize
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        TerminalBounds::new(
            px(20.0),
            px(8.0),
            Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(800.0),
                    height: px(600.0),
                },
            },
        )
    }
}

impl From<TerminalBounds> for WindowSize {
    fn from(val: TerminalBounds) -> Self {
        WindowSize {
            num_lines: val.num_lines() as u16,
            num_cols: val.num_columns() as u16,
            cell_width: f32::from(val.cell_width) as u16,
            cell_height: f32::from(val.line_height) as u16,
        }
    }
}

impl Dimensions for TerminalBounds {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

/// Listener for Alacritty events
#[derive(Clone)]
pub struct ZedListener(pub UnboundedSender<AlacTermEvent>);

impl EventListener for ZedListener {
    fn send_event(&self, event: AlacTermEvent) {
        self.0.unbounded_send(event).ok();
    }
}

/// A single terminal session with PTY
pub struct TerminalSession {
    pub id: usize,
    pub name: String,
    term: Arc<FairMutex<Term<ZedListener>>>,
    pty_tx: Notifier,
    _io_thread: Option<std::thread::JoinHandle<()>>,
    bounds: TerminalBounds,
    title: String,
    pub last_content: TerminalContent,  // Cache like Zed does
    events: VecDeque<AlacTermEvent>,    // Event queue
}

impl TerminalSession {
    pub fn new_with_sender(id: usize, name: String, working_directory: Option<PathBuf>, events_tx: UnboundedSender<AlacTermEvent>, cx: &mut App) -> Result<Self> {
        // Create Alacritty configuration
        let config = Config {
            scrolling_history: 10000,
            ..Config::default()
        };

        // Create the terminal with default bounds
        let term = Term::new(
            config.clone(),
            &TerminalBounds::default(),
            ZedListener(events_tx.clone()),
        );

        let term = Arc::new(FairMutex::new(term));

        // Determine shell to use
        let shell = if cfg!(target_os = "windows") {
            // Try PowerShell first, fallback to cmd
            if let Ok(pwsh_path) = which::which("pwsh") {
                tty::Shell::new(pwsh_path.to_string_lossy().to_string(), vec![])
            } else if let Ok(ps_path) = which::which("powershell") {
                tty::Shell::new(ps_path.to_string_lossy().to_string(), vec![])
            } else {
                tty::Shell::new("cmd".to_string(), vec![])
            }
        } else {
            // Try bash first, fallback to sh
            if let Ok(bash_path) = which::which("bash") {
                tty::Shell::new(bash_path.to_string_lossy().to_string(), vec![])
            } else {
                tty::Shell::new("sh".to_string(), vec![])
            }
        };

        // Set up PTY options
        let pty_options = tty::Options {
            shell: Some(shell),
            working_directory: working_directory.clone(),
            drain_on_exit: true,
            env: Default::default(),
        };

        // Create PTY
        let pty = tty::new(&pty_options, TerminalBounds::default().into(), 0)
            .context("failed to create PTY")?;

        // Connect terminal and PTY via event loop
        let event_loop = EventLoop::new(
            term.clone(),
            ZedListener(events_tx),
            pty,
            true, // drain_on_exit
            false, // hold
        )
        .context("failed to create event loop")?;

        let pty_tx = event_loop.channel();
        let _io_thread = event_loop.spawn();

        Ok(Self {
            id,
            name: name.clone(),
            term,
            pty_tx: Notifier(pty_tx),
            _io_thread: None,
            bounds: TerminalBounds::default(),
            title: name,
            last_content: Default::default(),
            events: VecDeque::with_capacity(10),
        })
    }
    
    /*
    /// Subscribe to terminal events (Zed approach) - DISABLED due to async API mismatch
    pub fn subscribe(mut events_rx: UnboundedReceiver<AlacTermEvent>, entity: Entity<Terminal>, cx: &mut Context<Terminal>) {
        cx.spawn(|_terminal, mut cx| async move {
            use futures::StreamExt;
            
            // Wait for events from alacritty and notify GPUI to repaint
            while let Some(event) = events_rx.next().await {
                // Process event and trigger repaint
                _ = entity.update(&mut cx, |terminal, cx| -> Result<()> {
                    if let Some(session) = terminal.active_session_mut() {
                        match event {
                            AlacTermEvent::Wakeup => {
                                // Terminal content changed - trigger repaint
                                cx.notify();
                            }
                            AlacTermEvent::Title(title) => {
                                session.title = title;
                                cx.notify();
                            }
                            AlacTermEvent::Bell => {
                                cx.notify();
                            }
                            AlacTermEvent::Exit => {
                                cx.notify();
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                });
            }
            anyhow::Ok(())
        }).detach();
    }
    */

    /// Set terminal size (from Zed)
    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        if self.last_content.terminal_bounds != new_bounds {
            self.bounds = new_bounds;
            self.last_content.terminal_bounds = new_bounds;
            let window_size = WindowSize::from(new_bounds);
            self.term.lock().resize(new_bounds);
            self.pty_tx.0.send(Msg::Resize(window_size)).ok();
        }
    }

    /// Sync terminal content (from Zed)
    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Terminal>) {
        // Update last_content from terminal (EXACT Zed approach)
        let term = self.term.lock();
        let old_content = self.last_content.clone();
        self.last_content = Self::make_content(&term, &self.last_content);
        
        // Check if content actually changed to trigger repaint
        let content_changed = old_content.cells.len() != self.last_content.cells.len()
            || old_content.cursor.point != self.last_content.cursor.point;
            
        if content_changed {
            drop(term); // Release the lock before notifying
            cx.notify();
        }
    }

    /// Make terminal content from Alacritty term (from Zed - EXACT copy)
    fn make_content(
        term: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, Term<ZedListener>>,
        last_content: &TerminalContent
    ) -> TerminalContent {
        let content = term.renderable_content();

        // Pre-allocate with estimated size to reduce reallocations
        let estimated_size = content.display_iter.size_hint().0;
        let mut cells = Vec::with_capacity(estimated_size);

        cells.extend(content.display_iter.map(|ic| IndexedCell {
            point: ic.point,
            cell: ic.cell.clone(),
        }));

        // Get cursor char
        let cursor_char = {
            let cursor_point = content.cursor.point;
            cells.iter()
                .find(|ic| ic.point.line == cursor_point.line && ic.point.column == cursor_point.column)
                .map(|ic| ic.c)
                .unwrap_or(' ')
        };

        TerminalContent {
            cells,
            mode: content.mode,
            display_offset: content.display_offset,
            cursor: content.cursor,
            cursor_char,
            terminal_bounds: last_content.terminal_bounds, // Keep existing bounds
        }
    }

    /// Send input to the terminal
    pub fn send_input(&mut self, input: &str) {
        let bytes = input.as_bytes().to_vec();
        self.pty_tx.0.send(Msg::Input(bytes.into())).ok();
    }

    /// Get the terminal for rendering
    pub fn term(&self) -> &Arc<FairMutex<Term<ZedListener>>> {
        &self.term
    }

    /// Process pending events (deprecated - events are now handled via subscribe)
    pub fn process_events(&mut self, cx: &mut Context<Terminal>) -> Vec<Event> {
        // Events are now processed in the subscribe method
        Vec::new()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn clear(&mut self) {
        // Clear the terminal by writing ANSI escape sequence
        self.send_input("\x1b[2J\x1b[H");
    }
    
    /// Scroll the terminal - from Zed
    pub fn scroll(&mut self, scroll: alacritty_terminal::grid::Scroll) {
        let mut term = self.term.lock();
        term.scroll_display(scroll);
    }
    
    /// Send mouse event to terminal - from Zed
    pub fn mouse_event(&mut self, point: AlacPoint, button: MouseButton, pressed: bool, sgr_mode: bool) {
        if sgr_mode {
            // SGR extended mouse mode (CSI < ... M or m)
            // Format: ESC[<button;x;y(M for press, m for release)
            let button_code: u8 = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };
            
            let x = point.column.0 + 1;
            let y = point.line.0 + 1;
            let event_char = if pressed { 'M' } else { 'm' };
            
            let sequence = format!("\x1b[<{};{};{}{}", button_code, x, y, event_char);
            self.send_input(&sequence);
        } else {
            // Simple mouse reporting for X10 mouse mode
            // Format: ESC[M<button><x+32><y+32>
            let button_code: u8 = match button {
                MouseButton::Left => if pressed { 0 } else { 3 },
                MouseButton::Middle => if pressed { 1 } else { 3 },
                MouseButton::Right => if pressed { 2 } else { 3 },
                _ => return,
            };
            
            let x = (point.column.0 as u8 + 32 + 1).min(255);
            let y = (point.line.0 as u8 + 32 + 1).min(255);
            
            let bytes = vec![27, b'[', b'M', button_code + 32, x, y];
            self.pty_tx.notify(bytes);
        }
    }
    
    /// Send mouse move event to terminal - from Zed
    pub fn mouse_move_event(&mut self, point: AlacPoint, sgr_mode: bool) {
        if sgr_mode {
            // SGR extended mouse mode (CSI < ... M)
            // Format: ESC[<button;x;y M (button 35 is move with no button)
            let button_code: u8 = 35; // Move with no button
            let x = point.column.0 + 1;
            let y = point.line.0 + 1;
            
            let sequence = format!("\x1b[<{};{};{}M", button_code, x, y);
            self.send_input(&sequence);
        } else {
            // X10 mouse move report
            let button_code: u8 = 32 + 3; // Move with no button
            let x = (point.column.0 as u8 + 32 + 1).min(255);
            let y = (point.line.0 as u8 + 32 + 1).min(255);
            
            let bytes = vec![27, b'[', b'M', button_code, x, y];
            self.pty_tx.notify(bytes);
        }
    }
    
    /// Send mouse scroll event to terminal - for programs like btop
    pub fn mouse_scroll_event(&mut self, point: AlacPoint, scroll_up: bool) {
        // Mouse wheel report using SGR extended mode (CSI < ... M)
        // Button 64 = scroll up, 65 = scroll down
        let button = if scroll_up { 64 } else { 65 };
        let x = point.column.0 + 1;
        let y = point.line.0 + 1;
        
        // SGR extended mouse mode
        let sequence = format!("\x1b[<{};{};{}M", button, x, y);
        self.send_input(&sequence);
    }
}

/// Terminal with multiple sessions
pub struct Terminal {
    focus_handle: FocusHandle,
    sessions: Vec<TerminalSession>,
    active_session: usize,
    next_session_id: usize,
}

impl Terminal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Result<Self> {
        let focus_handle = cx.focus_handle();
        let mut terminal = Self {
            focus_handle,
            sessions: Vec::new(),
            active_session: 0,
            next_session_id: 0,
        };

        // Create initial session and get events channel
        let events_rx = terminal.add_session(None, cx)?;
        
        // Subscribe to events (EXACT Zed pattern) - this is the key to real-time updates!
        cx.spawn(async move |terminal, cx| {
            use futures::StreamExt;
            use futures::FutureExt;
            let mut events_rx = events_rx;
            
            // Wait for events from alacritty and notify GPUI to repaint (EXACT pattern from our codebase)
            while let Some(event) = events_rx.next().await {
                // Process the first event immediately for lowered latency
                cx.update(|cx| {
                    terminal.update(cx, |terminal, cx| {
                        if let Some(session) = terminal.active_session_mut() {
                            Self::process_terminal_event(session, &event, cx);
                        }
                    }).ok();
                }).ok();

                // Batch additional events for efficiency (from Zed)
                'outer: loop {
                    let mut events = Vec::new();
                    let mut timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(4))
                        .fuse();
                    let mut wakeup = false;
                    
                    loop {
                        futures::select_biased! {
                            _ = timer => break,
                            event = events_rx.next() => {
                                if let Some(event) = event {
                                    if matches!(event, AlacTermEvent::Wakeup) {
                                        wakeup = true;
                                    } else {
                                        events.push(event);
                                    }

                                    if events.len() > 100 {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            },
                        }
                    }

                    if events.is_empty() && !wakeup {
                        smol::future::yield_now().await;
                        break 'outer;
                    }

                    cx.update(|cx| {
                        terminal.update(cx, |terminal, cx| {
                            if let Some(session) = terminal.active_session_mut() {
                                if wakeup {
                                    Self::process_terminal_event(session, &AlacTermEvent::Wakeup, cx);
                                }

                                for event in events {
                                    Self::process_terminal_event(session, &event, cx);
                                }
                            }
                        }).ok();
                    }).ok();
                    smol::future::yield_now().await;
                }
            }

            anyhow::Ok(())
        })
        .detach();

        Ok(terminal)
    }

    /// Process a single terminal event (extracted for reuse)
    fn process_terminal_event(session: &mut TerminalSession, event: &AlacTermEvent, cx: &mut Context<Terminal>) {
        match event {
            AlacTermEvent::Wakeup => {
                // Terminal content changed - trigger repaint
                cx.notify();
            }
            AlacTermEvent::Title(title) => {
                session.title = title.clone();
                cx.notify();
            }
            AlacTermEvent::Bell => {
                cx.notify();
            }
            AlacTermEvent::Exit => {
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn add_session(&mut self, working_directory: Option<PathBuf>, cx: &mut Context<Self>) -> Result<UnboundedReceiver<AlacTermEvent>> {
        let session_name = format!("Terminal {}", self.next_session_id + 1);
        
        // Set up event communication BEFORE creating session
        let (events_tx, events_rx) = unbounded();
        
        // Create session with events_tx
        let session = TerminalSession::new_with_sender(self.next_session_id, session_name, working_directory, events_tx, cx)?;

        self.sessions.push(session);
        self.active_session = self.sessions.len() - 1;
        self.next_session_id += 1;
        cx.notify();

        Ok(events_rx)
    }

    pub fn close_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.sessions.len() <= 1 {
            return; // Keep at least one session
        }

        self.sessions.remove(index);

        if self.active_session >= self.sessions.len() {
            self.active_session = self.sessions.len() - 1;
        } else if index < self.active_session {
            self.active_session -= 1;
        }

        cx.notify();
    }

    pub fn switch_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.sessions.len() {
            self.active_session = index;
            cx.notify();
        }
    }

    pub fn active_session(&self) -> Option<&TerminalSession> {
        self.sessions.get(self.active_session)
    }

    pub fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(self.active_session)
    }
    
    /// Scroll terminal up (into history) - from Zed
    pub fn scroll_up(&mut self, lines: usize, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            session.scroll(alacritty_terminal::grid::Scroll::Delta(lines as i32));
            // Immediately update content to reflect scroll position
            let term = session.term.lock();
            session.last_content = TerminalSession::make_content(&term, &session.last_content);
            drop(term);
            cx.notify();
        }
    }
    
    /// Scroll terminal down (towards bottom) - from Zed
    pub fn scroll_down(&mut self, lines: usize, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            session.scroll(alacritty_terminal::grid::Scroll::Delta(-(lines as i32)));
            // Immediately update content to reflect scroll position
            let term = session.term.lock();
            session.last_content = TerminalSession::make_content(&term, &session.last_content);
            drop(term);
            cx.notify();
        }
    }
    
    /// Mouse down event - from Zed
    pub fn mouse_down(&mut self, event: &MouseDownEvent, origin: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            let position = event.position - origin;
            let point = grid_point(
                position,
                &session.last_content.terminal_bounds,
                session.last_content.display_offset,
            );
            
            // If terminal is in mouse mode, send mouse event to terminal
            if session.last_content.mode.intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE) && !event.modifiers.shift {
                let sgr_mode = session.last_content.mode.contains(TermMode::SGR_MOUSE);
                session.mouse_event(point, event.button, true, sgr_mode);
            }
            cx.notify();
        }
    }
    
    /// Mouse up event - from Zed
    pub fn mouse_up(&mut self, event: &MouseUpEvent, origin: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            let position = event.position - origin;
            let point = grid_point(
                position,
                &session.last_content.terminal_bounds,
                session.last_content.display_offset,
            );
            
            // If terminal is in mouse mode, send mouse event to terminal
            if session.last_content.mode.intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE) && !event.modifiers.shift {
                let sgr_mode = session.last_content.mode.contains(TermMode::SGR_MOUSE);
                session.mouse_event(point, event.button, false, sgr_mode);
            }
            cx.notify();
        }
    }
    
    /// Mouse move event - from Zed
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, origin: Point<Pixels>) {
        if let Some(session) = self.active_session_mut() {
            let position = event.position - origin;
            let point = grid_point(
                position,
                &session.last_content.terminal_bounds,
                session.last_content.display_offset,
            );
            
            // If terminal is in mouse mode, send mouse event to terminal
            if session.last_content.mode.intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE) && !event.modifiers.shift {
                let sgr_mode = session.last_content.mode.contains(TermMode::SGR_MOUSE);
                // Mouse move in mouse mode
                session.mouse_move_event(point, sgr_mode);
            }
        }
    }
    
    /// Mouse scroll event - for programs like btop
    pub fn mouse_scroll(&mut self, event: &ScrollWheelEvent, origin: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            // Check if terminal is in mouse mode - if so, send scroll to the application
            if session.last_content.mode.intersects(TermMode::MOUSE_MODE | TermMode::SGR_MOUSE) && !event.modifiers.shift {
                // Terminal application wants mouse events - send scroll to it
                let position = event.position - origin;
                let point = grid_point(
                    position,
                    &session.last_content.terminal_bounds,
                    session.last_content.display_offset,
                );
                
                let delta_y = event.delta.pixel_delta(px(20.0)).y;
                if delta_y != px(0.0) {
                    session.mouse_scroll_event(point, delta_y > px(0.0));
                }
                cx.notify();
            } else {
                // Normal scroll - scroll the terminal display
                let delta_y = event.delta.pixel_delta(px(20.0)).y;
                let lines_to_scroll = (delta_y / session.last_content.terminal_bounds.line_height).abs() as usize;
                
                if lines_to_scroll > 0 {
                    if delta_y > px(0.0) {
                        // Scroll up (into history)
                        self.scroll_up(lines_to_scroll, cx);
                    } else {
                        // Scroll down
                        self.scroll_down(lines_to_scroll, cx);
                    }
                }
            }
        }
    }

    pub fn sessions(&self) -> &[TerminalSession] {
        &self.sessions
    }

    pub fn handle_input(&mut self, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            // Regular text input - send as bytes
            session.send_input(text);
            cx.notify();
        }
    }

    /// Handle Tab key action (prevents focus navigation)
    pub fn send_tab(&mut self, _action: &SendTab, _window: &mut Window, cx: &mut Context<Self>) {
        // Use the keystroke system to properly handle tab
        let tab_keystroke = gpui::Keystroke::parse("tab").unwrap();
        self.try_keystroke(&tab_keystroke, false, cx);
    }

    /// Handle Shift+Tab key action (prevents focus navigation)
    pub fn send_shift_tab(&mut self, _action: &SendShiftTab, _window: &mut Window, cx: &mut Context<Self>) {
        // Use the keystroke system to properly handle shift+tab
        let shift_tab_keystroke = gpui::Keystroke::parse("shift-tab").unwrap();
        self.try_keystroke(&shift_tab_keystroke, false, cx);
    }

    pub fn try_keystroke(&mut self, keystroke: &Keystroke, alt_is_meta: bool, cx: &mut Context<Self>) -> bool {
        if let Some(session) = self.active_session_mut() {
            // Convert keystroke to escape sequence (from Zed's to_esc_str)
            let esc = super::mappings::keys::to_esc_str(keystroke, &session.last_content.mode, alt_is_meta);
            if let Some(esc) = esc {
                match esc {
                    std::borrow::Cow::Borrowed(string) => session.send_input(string),
                    std::borrow::Cow::Owned(string) => session.send_input(&string),
                };
                cx.notify();
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn update_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session_mut() {
            // Update terminal size if needed
            // Sync terminal content
            session.sync(window, cx);
        }
    }
}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<Event> for Terminal {}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{v_flex, h_flex, StyledExt, ActiveTheme, button::{Button, ButtonVariants}, IconName, Sizable};
        use super::terminal_element::TerminalElement;

        // Update terminal content before rendering
        self.update_events(window, cx);
        
        let active_session = self.active_session().map(|s| s.name.clone()).unwrap_or_default();
        let is_focused = self.focus_handle.is_focused(window);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Header
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(active_session)
                            )
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("clear-terminal")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Trash)
                                    .tooltip("Clear Terminal")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        if let Some(session) = this.active_session_mut() {
                                            session.clear();
                                        }
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("new-terminal")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip("New Terminal")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        if let Err(e) = this.add_session(None, cx) {
                                            eprintln!("Failed to create new terminal: {}", e);
                                        }
                                    }))
                            )
                    )
            )
            .child(
                // Terminal sessions tabs
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .gap_1()
                    .children(
                        self.sessions.iter().enumerate().map(|(index, session)| {
                            let is_active = index == self.active_session;
                            let has_multiple = self.sessions.len() > 1;

                            let mut tab = h_flex()
                                .px_3()
                                .py_1()
                                .rounded(px(4.0))
                                .gap_2()
                                .items_center()
                                .cursor_pointer();
                            
                            if is_active {
                                tab = tab.bg(cx.theme().primary)
                                    .text_color(cx.theme().primary_foreground);
                            } else {
                                tab = tab.hover(|this| this.bg(cx.theme().accent));
                            }
                            
                            tab = tab.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                this.switch_session(index, cx);
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .child(session.name.clone())
                            );
                            
                            if has_multiple {
                                tab = tab.child(
                                    div()
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.close_session(index, cx);
                                        }))
                                        .child(
                                            ui::Icon::new(IconName::Close)
                                                .size_3()
                                        )
                                );
                            }
                            
                            tab
                        })
                    )
            )
            .child(
                // Terminal rendering
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .on_action(cx.listener(Terminal::send_tab))  // Handle Tab action to prevent focus navigation
                    .on_action(cx.listener(Terminal::send_shift_tab))  // Handle Shift+Tab action
                    .child(TerminalElement::new(
                        cx.entity().clone(),
                        self.focus_handle.clone(),
                        is_focused
                    ))
            )
    }
}

/// Convert pixel position to grid point - from Zed
fn grid_point(
    position: Point<Pixels>,
    bounds: &TerminalBounds,
    display_offset: usize,
) -> AlacPoint {
    let col = (position.x / bounds.cell_width).floor() as usize;
    let line = (position.y / bounds.line_height).floor() as i32 - display_offset as i32;
    
    // Calculate max columns from bounds
    let max_col = ((bounds.bounds.size.width / bounds.cell_width).floor() as usize).saturating_sub(1);
    
    AlacPoint::new(
        Line(line),
        Column(col.min(max_col)),
    )
}
