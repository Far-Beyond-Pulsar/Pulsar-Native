use std::sync::Arc;

use gpui::{
    anchored, deferred, div, img, point, prelude::FluentBuilder as _, px, AnyElement, App,
    ClickEvent, Context, Corner, EventEmitter, ImageSource, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, RenderImage, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window,
};
use ui::{
    button::Button,
    h_flex, v_flex, ActiveTheme as _, Icon, IconName,
};

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by ProfileDropdown that parents may subscribe to.
pub enum ProfileDropdownEvent {
    /// User clicked "Sign In with GitHub" — parent should handle the sign-in flow.
    SignInRequested,
    /// User signed out — profile cleared, avatar reset.
    SignedOut,
    /// User clicked "Multiplayer Sessions" — parent should open the friends/invite UI.
    MultiplayerSessionsRequested,
}

// ── Component ─────────────────────────────────────────────────────────────────

/// A self-contained profile avatar button + rich dropdown card.
///
/// Drop one `Entity<ProfileDropdown>` into any titlebar; subscribe to
/// [`ProfileDropdownEvent::SignInRequested`] to begin the sign-in flow for that
/// screen (entry screen uses device-flow; editor uses launcher).
pub struct ProfileDropdown {
    pub avatar_image: Option<Arc<RenderImage>>,
    pub avatar_url_loaded: Option<String>,
    /// Whether the dropdown panel is currently visible.
    pub is_open: bool,
}

impl EventEmitter<ProfileDropdownEvent> for ProfileDropdown {}

impl ProfileDropdown {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            avatar_image: None,
            avatar_url_loaded: None,
            is_open: false,
        };
        // Kick off eager avatar load so the trigger shows an image immediately.
        this.ensure_avatar_loaded(cx);
        this
    }

    /// Check if the profile's avatar URL has changed and start an async load
    /// if needed.  Call this every render frame (it no-ops when nothing changed).
    pub fn ensure_avatar_loaded(&mut self, cx: &mut Context<Self>) {
        let profile =
            engine_state::EngineContext::global().and_then(|ec| ec.auth_profile());

        let url = match profile.and_then(|p| p.avatar_url) {
            Some(u) => u,
            None => {
                self.avatar_image = None;
                self.avatar_url_loaded = None;
                return;
            }
        };

        if self.avatar_url_loaded.as_deref() == Some(url.as_str()) {
            return; // already loading / loaded this URL
        }

        self.avatar_url_loaded = Some(url.clone());
        self.avatar_image = None;

        let (tx, rx) = smol::channel::bounded::<Option<Arc<RenderImage>>>(1);
        std::thread::spawn(move || {
            let image = fetch_avatar_image(&url).ok();
            let _ = smol::block_on(tx.send(image));
        });

        cx.spawn(async move |this, cx| {
            if let Ok(maybe_image) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.avatar_image = maybe_image;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    // ── Private render helpers ─────────────────────────────────────────────

    fn render_trigger(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile =
            engine_state::EngineContext::global().and_then(|ec| ec.auth_profile());
        let login = profile.as_ref().map(|p| p.login.as_str()).unwrap_or("?");
        let initial = login
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        let accent = cx.theme().accent;

        let avatar: AnyElement = if let Some(render_img) = self.avatar_image.clone() {
            div()
                .w(px(26.))
                .h(px(26.))
                .rounded_full()
                .overflow_hidden()
                .child(
                    img(ImageSource::Render(render_img))
                        .w_full()
                        .h_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            div()
                .w(px(26.))
                .h(px(26.))
                .rounded_full()
                .bg(accent.opacity(0.18))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(accent)
                        .child(initial),
                )
                .into_any_element()
        };

        div()
            .id("profile-dropdown-trigger")
            .cursor_pointer()
            .rounded_full()
            .p(px(1.))
            .hover(|s| s.bg(cx.theme().accent.opacity(0.1)))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.is_open = !this.is_open;
                cx.notify();
            }))
            .child(avatar)
    }

    fn render_menu_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile =
            engine_state::EngineContext::global().and_then(|ec| ec.auth_profile());
        let theme = cx.theme();

        v_flex()
            .w(px(280.))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .map(|this| match profile {
                Some(ref p) => this.child(self.render_signed_in(p, cx)),
                None => this.child(self.render_signed_out(cx)),
            })
    }

    fn render_signed_in(
        &self,
        profile: &engine_state::AuthProfile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let login = profile.login.clone();
        let display = profile
            .display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| login.clone());
        let theme = cx.theme();
        let accent = theme.accent;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;

        // Large avatar (56 px) inside a white ring, overlapping the accent banner
        let large_avatar: AnyElement = if let Some(img_data) = self.avatar_image.clone() {
            div()
                .w(px(56.))
                .h(px(56.))
                .rounded_full()
                .overflow_hidden()
                .flex_shrink_0()
                .child(
                    img(ImageSource::Render(img_data))
                        .w_full()
                        .h_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            let init = login
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            div()
                .w(px(56.))
                .h(px(56.))
                .rounded_full()
                .bg(accent.opacity(0.18))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(accent)
                        .child(init),
                )
                .into_any_element()
        };

        let login_profile = login.clone();
        let login_repos = login.clone();
        let login_copy = login.clone();

        v_flex()
            // ── Banner + avatar ──────────────────────────────────
            .child(
                div()
                    .h(px(68.))
                    .relative()
                    .overflow_hidden()
                    .child(div().absolute().inset_0().bg(accent.opacity(0.15)))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(-16.))
                            .left(px(16.))
                            .p(px(3.))
                            .rounded_full()
                            .bg(theme.popover)
                            .child(large_avatar),
                    ),
            )
            // ── Identity ─────────────────────────────────────────
            .child(
                v_flex()
                    .pt(px(24.))
                    .px_4()
                    .pb_3()
                    .gap_0p5()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(display),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("@{login}")),
                    ),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── GitHub section ────────────────────────────────────
            .child(
                v_flex()
                    .p_2()
                    .gap_0p5()
                    .child(section_label("GITHUB", muted))
                    .child(menu_row(
                        IconName::Github,
                        "Open GitHub Profile",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.open_url(&format!("https://github.com/{login_profile}"));
                            this.is_open = false;
                            cx.notify();
                        }),
                    ))
                    .child(menu_row(
                        IconName::GitFork,
                        "View Repositories",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.open_url(&format!(
                                "https://github.com/{login_repos}?tab=repositories"
                            ));
                            this.is_open = false;
                            cx.notify();
                        }),
                    ))
                    .child(menu_row(
                        IconName::Copy,
                        "Copy Username",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                format!("@{login_copy}"),
                            ));
                            this.is_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── Git Identity section ──────────────────────────────
            .child(
                v_flex()
                    .p_2()
                    .gap_0p5()
                    .child(section_label("GIT IDENTITY", muted))
                    .child(menu_row(
                        IconName::GitCommit,
                        "Configure Git Author",
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            // TODO: open git identity settings panel
                            this.is_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── Collaboration section ─────────────────────────────
            .child(
                v_flex()
                    .p_2()
                    .gap_0p5()
                    .child(section_label("COLLABORATION", muted))
                    .child(menu_row(
                        IconName::Group,
                        "Multiplayer Sessions",
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.is_open = false;
                            cx.emit(ProfileDropdownEvent::MultiplayerSessionsRequested);
                            cx.notify();
                        }),
                    )),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── Sign Out ──────────────────────────────────────────
            .child(
                v_flex().p_2().child(menu_row(
                    IconName::LogOut,
                    "Sign Out",
                    true,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        let _ = pulsar_auth::sign_out();
                        if let Some(ec) = engine_state::EngineContext::global() {
                            ec.clear_auth_profile();
                        }
                        this.avatar_image = None;
                        this.avatar_url_loaded = None;
                        this.is_open = false;
                        cx.emit(ProfileDropdownEvent::SignedOut);
                        cx.notify();
                    }),
                )),
            )
    }

    fn render_signed_out(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;

        v_flex()
            .p_4()
            .gap_3()
            // ── Guest header ─────────────────────────────────────
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .w(px(48.))
                            .h(px(48.))
                            .rounded_full()
                            .bg(theme.muted.opacity(0.4))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .child(
                                Icon::new(IconName::ProfileCircle)
                                    .size(px(22.))
                                    .text_color(muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Guest"),
                            )
                            .child(div().text_xs().text_color(muted).child("Not signed in")),
                    ),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── Benefits ─────────────────────────────────────────
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Sign in to unlock:"),
                    )
                    .child(benefit_row("Git version control & author identity"))
                    .child(benefit_row("Multiplayer real-time sessions"))
                    .child(benefit_row("Cloud project sync"))
                    .child(benefit_row("FAB asset marketplace")),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            // ── Sign In button ────────────────────────────────────
            .child(
                Button::new("profile-sign-in")
                    .w_full()
                    .label("Sign In with GitHub")
                    .icon(IconName::Github)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.is_open = false;
                        cx.emit(ProfileDropdownEvent::SignInRequested);
                        cx.notify();
                    })),
            )
    }
}

impl Render for ProfileDropdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_avatar_loaded(cx);

        let vp = window.viewport_size();
        let is_open = self.is_open;

        div()
            .child(self.render_trigger(cx))
            .when(is_open, |this| {
                // Render the dropdown panel in a deferred z-layer so it floats
                // above all other content, positioned relative to the window.
                this.child(
                    deferred(
                        anchored()
                            .anchor(Corner::TopRight)
                            .position(point(vp.width - px(8.), px(34.)))
                            .child(
                                div()
                                    .occlude()
                                    .on_mouse_down_out(cx.listener(
                                        |this, _: &gpui::MouseDownEvent, _, cx| {
                                            this.is_open = false;
                                            cx.notify();
                                        },
                                    ))
                                    .child(self.render_menu_panel(cx)),
                            ),
                    )
                    .with_priority(1),
                )
            })
    }
}

// ── Free-standing helpers ─────────────────────────────────────────────────────

fn section_label(text: &str, color: gpui::Hsla) -> impl IntoElement {
    div()
        .px_2()
        .py(px(4.))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.to_string())
}

fn menu_row(
    icon: IconName,
    label: &str,
    destructive: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    h_flex()
        .id(gpui::ElementId::Name(label.clone().into()))
        .w_full()
        .gap_2()
        .items_center()
        .px_2()
        .py(px(6.))
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(gpui::Hsla::default().opacity(0.06)))
        .on_click(handler)
        .child(
            Icon::new(icon).size(px(14.)).map(|i| {
                if destructive {
                    i.text_color(gpui::red())
                } else {
                    i
                }
            }),
        )
        .child(
            div()
                .text_sm()
                .map(|d| if destructive { d.text_color(gpui::red()) } else { d })
                .child(label),
        )
}

fn benefit_row(text: &str) -> impl IntoElement {
    let text = text.to_string();
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(IconName::Check).size(px(12.)))
        .child(div().text_xs().child(text))
}

/// Download a GitHub avatar PNG and decode it into a [`RenderImage`].
pub(crate) fn fetch_avatar_image(url: &str) -> Result<Arc<RenderImage>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Pulsar-Native/1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|e| format!("decode: {e}"))?
        .into_rgba8();
    let frame = image::Frame::new(rgba);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
