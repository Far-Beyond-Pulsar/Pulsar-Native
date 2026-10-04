use rust_i18n::t;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    actions, div, prelude::FluentBuilder as _, px, AnyElement, App, AppContext, ClickEvent,
    Context, Corner, Entity, FocusHandle, InteractiveElement as _, IntoElement, Menu, MenuItem,
    MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window,
};
use ui::{
    badge::Badge,
    button::{Button, ButtonVariants as _},
    h_flex, locale,
    menu::AppMenuBar,
    popup_menu::PopupMenuExt as _,
    scroll::ScrollbarShow,
    v_flex, ActiveTheme as _, ContextModal as _, IconName, PixelsExt, Sizable as _, Theme,
    ThemeMode, TitleBar,
};

mod dev_popover;
use crate::profile_dropdown::ProfileDropdownEvent;
use dev_popover::DevPopover;
use ui::themes::ThemeSwitcher;

// Define UI preference actions
#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = ui, no_json)]
pub struct SelectFont(pub i32);

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = ui, no_json)]
pub struct SelectLocale(pub String);

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = ui, no_json)]
pub struct SelectRadius(pub i32);

#[derive(gpui::Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = ui, no_json)]
pub struct SelectScrollbarShow(pub ScrollbarShow);

// Define actions for the main menu
actions!(
    menu,
    [
        // App menu
        AboutApp,
        CheckUpdates,
        Preferences,
        Settings,
        Hide,
        HideOthers,
        ShowAll,
        QuitApp,
        // File menu
        NewFile,
        NewWindow,
        NewProject,
        NewScene,
        NewScript,
        NewShader,
        NewMaterial,
        NewPrefab,
        NewBlueprint,
        NewComponent,
        NewSystem,
        OpenFile,
        OpenFolder,
        OpenRecent,
        OpenRecentFiles,
        ClearRecent,
        SaveFile,
        SaveAs,
        SaveAll,
        SaveWorkspace,
        ImportAsset,
        ImportModel,
        ImportTexture,
        ImportAudio,
        BatchImport,
        ImportFromUnity,
        ImportFromUnreal,
        ImportFromGodot,
        ExportBuild,
        ExportScene,
        ExportSelected,
        ExportWindows,
        ExportLinux,
        ExportMacOS,
        ExportWeb,
        ExportAndroid,
        ExportIOS,
        RevertFile,
        CloseFile,
        CloseFolder,
        CloseAll,
        CloseOthers,
        // Edit menu
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Delete,
        SelectAll,
        SelectNone,
        Find,
        FindNext,
        FindPrevious,
        FindReplace,
        ReplaceNext,
        ReplaceAll,
        FindInFiles,
        ReplaceInFiles,
        FindUsages,
        FindImplementations,
        FormatDocument,
        FormatSelection,
        CommentLine,
        UncommentLine,
        ToggleComment,
        Fold,
        Unfold,
        FoldAll,
        UnfoldAll,
        SortLines,
        RemoveDuplicates,
        TrimWhitespace,
        // Selection menu
        SelectLine,
        SelectWord,
        SelectScope,
        ExpandSelection,
        ShrinkSelection,
        AddCursorAbove,
        AddCursorBelow,
        AddCursorLineEnds,
        SelectAllOccurrences,
        SelectNextOccurrence,
        SkipOccurrence,
        // View menu
        ToggleExplorer,
        ToggleHierarchy,
        ToggleInspector,
        ToggleAssetBrowser,
        ToggleConsole,
        ToggleTerminal,
        ToggleOutput,
        ToggleProblems,
        ToggleDebug,
        ToggleProfiler,
        ToggleMemoryAnalyzer,
        ToggleNetwork,
        SplitHorizontal,
        SplitVertical,
        SingleColumn,
        TwoColumns,
        ThreeColumns,
        ResetLayout,
        SaveLayout,
        CommandPalette,
        QuickOpen,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleMinimap,
        ToggleLineNumbers,
        ToggleBreadcrumbs,
        ToggleWhitespace,
        ToggleFullscreen,
        ToggleZenMode,
        // Go menu
        GoToFile,
        GoToSymbol,
        GoToLine,
        GoToDefinition,
        GoToTypeDefinition,
        GoToImplementation,
        GoToReferences,
        GoBack,
        GoForward,
        GoToLastEdit,
        NextProblem,
        PreviousProblem,
        // Project menu
        ProjectSettings,
        BuildSettings,
        PackageSettings,
        AddDependency,
        UpdateDependencies,
        RemoveUnusedDeps,
        OpenCargoToml,
        GenerateDocs,
        RunCargoCheck,
        RunClippy,
        FormatProject,
        // Build menu
        Build,
        BuildAndRun,
        BuildRelease,
        Rebuild,
        Clean,
        BuildDebug,
        BuildReleaseDebug,
        BuildProfile,
        CancelBuild,
        ShowBuildOutput,
        // Run menu
        RunProject,
        RunWithoutDebug,
        StopExecution,
        RestartExecution,
        DebugProject,
        StartDebug,
        StopDebug,
        RestartDebug,
        StepOver,
        StepInto,
        StepOut,
        Continue,
        ToggleBreakpoint,
        DisableBreakpoints,
        RemoveBreakpoints,
        RunTests,
        RunFailedTests,
        RunFileTests,
        DebugTest,
        RunCoverage,
        RunBenchmarks,
        CompareBenchmarks,
        ProfileBuild,
        // Engine menu (using unique names)
        OpenScene,
        SaveScene,
        SaveSceneAs,
        PlayScene,
        PauseScene,
        StopScene,
        SceneSettings,
        CreateEmpty,
        CreateCube,
        CreateSphere,
        CreateCapsule,
        CreateCylinder,
        CreatePlane,
        CreateTerrain,
        CreateSprite,
        CreateTilemap,
        CreateParticles2D,
        CreateDirectionalLight,
        CreatePointLight,
        CreateSpotLight,
        CreateAreaLight,
        CreateAudioSource,
        CreateAudioListener,
        CreateParticleSystem,
        CreateVFX,
        CreatePostProcessing,
        CreateCamera,
        CreateOrthoCamera,
        CreateCanvas,
        CreatePanel,
        CreateButton,
        CreateText,
        CreateImage,
        CreateSlider,
        CreateInputField,
        AddRigidbody,
        AddCollider,
        AddCharacterController,
        PhysicsSettings,
        CollisionMatrix,
        RenderSettings,
        LightingSettings,
        QualitySettings,
        BakeLighting,
        ClearBakedData,
        FrameDebugger,
        EngineProfiler,
        PackageManager,
        AssetStore,
        // Assets menu
        CreateAsset,
        CreateMaterial,
        CreateShader,
        CreateTexture,
        CreateAnimClip,
        CreateAnimController,
        CreateAudioMixer,
        CreateRenderTexture,
        CreatePrefab,
        CreateScriptableObject,
        RefreshAssets,
        ReimportAssets,
        ReimportAllAssets,
        FindAssetReferences,
        FindMissingReferences,
        // Tools menu
        CargoCommands,
        GenerateRustAnalyzer,
        ExpandMacro,
        ShowSyntaxTree,
        ShowHIR,
        InlineVariable,
        ExtractFunction,
        ExtractVariable,
        ShaderEditor,
        CompileShader,
        ShaderVariants,
        SPIRVDisassembly,
        AnimationWindow,
        AnimatorWindow,
        Timeline,
        MaterialEditor,
        TerrainTools,
        ParticleEditor,
        AudioMixerWindow,
        VersionControl,
        TaskManager,
        Extensions,
        // Window menu
        Minimize,
        Zoom,
        BringAllToFront,
        CloseWindow,
        // Help menu
        ShowDocumentation,
        ShowAPIReference,
        ShowTutorials,
        ShowShortcuts,
        ReportIssue,
        ViewLogs,
        ReleaseNotes,
        // Developer menu (source builds only)
        ShowTypeDebugger,
        ShowAgentChat,
        RevealProjectFolder,
        DevSaveAsDefaultLevel,
        DevReloadAssets,
        DevInspectEngineState,
        DevShowBuildInfo,
        DevOpenWorkspaceRoot,
    ]
);

/// Build the application menu list.
///
/// This is separated from [`init_app_menus`] so the same menu tree can be
/// built twice — once for the platform native menu bar (macOS) and once
/// converted to [`gpui::OwnedMenu`] for the custom in-window menu bar on
/// Windows / Linux where `cx.get_menus()` always returns `None`.
fn build_app_menus(title: SharedString) -> Vec<Menu> {
    // Only engine-global menus live here. Editors (level editor, blueprint
    // editor, ...) provide their own menus in their own top bar, so nothing
    // editor-specific (edit, selection, view, build, run, assets, ...) belongs
    // in this list. Build / Run live on the global toolbar.
    vec![
        // Pulsar (logo) menu
        Menu {
            name: title,
            items: vec![
                MenuItem::action(t!("Menu.App.AboutApp").to_string(), AboutApp),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.App.CheckUpdates").to_string(), CheckUpdates),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.App.Preferences").to_string(), Preferences),
                MenuItem::action(t!("Menu.App.Settings").to_string(), Settings),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.App.Hide").to_string(), Hide),
                MenuItem::action(t!("Menu.App.HideOthers").to_string(), HideOthers),
                MenuItem::action(t!("Menu.App.ShowAll").to_string(), ShowAll),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.App.QuitApp").to_string(), QuitApp),
            ],
        },
        // File: project-level operations
        Menu {
            name: t!("Menu.File").into(),
            items: vec![
                MenuItem::action(t!("Menu.File.NewProject").to_string(), NewProject),
                MenuItem::action(t!("Menu.File.OpenFolder").to_string(), OpenFolder),
                MenuItem::Submenu(Menu {
                    name: t!("Menu.File.OpenRecent").into(),
                    items: vec![
                        MenuItem::action(t!("Menu.File.RecentProjects").to_string(), OpenRecent),
                        MenuItem::separator(),
                        MenuItem::action(t!("Menu.File.ClearRecent").to_string(), ClearRecent),
                    ],
                }),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.File.SaveAll").to_string(), SaveAll),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.File.CloseFolder").to_string(), CloseFolder),
            ],
        },
        // View: panels and window-level display
        Menu {
            name: t!("Menu.View").into(),
            items: vec![
                MenuItem::action("File Explorer", ToggleExplorer),
                MenuItem::action("Problems", ToggleProblems),
                MenuItem::action("Agent Chat", ShowAgentChat),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.View.CommandPalette").to_string(), CommandPalette),
                MenuItem::separator(),
                MenuItem::action("Full Screen", ToggleFullscreen),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Reset Zoom", ResetZoom),
            ],
        },
        // Go: movement between files
        Menu {
            name: t!("Menu.Go").into(),
            items: vec![
                MenuItem::action("Back", GoBack),
                MenuItem::action("Forward", GoForward),
                MenuItem::separator(),
                MenuItem::action("Go to File…", GoToFile),
            ],
        },
        // Search
        Menu {
            name: "Search".into(),
            items: vec![
                MenuItem::action("Search Everything…", CommandPalette),
                MenuItem::action("Go to File…", GoToFile),
                MenuItem::separator(),
                MenuItem::action("Search Documentation", ShowDocumentation),
            ],
        },
        // Project: the project as a whole
        Menu {
            name: t!("Menu.Project").into(),
            items: vec![
                MenuItem::action(
                    t!("Menu.Project.ProjectSettings").to_string(),
                    ProjectSettings,
                ),
                MenuItem::action(t!("Menu.Project.OpenCargoToml").to_string(), OpenCargoToml),
                MenuItem::separator(),
                MenuItem::action("Reveal in File Explorer", RevealProjectFolder),
            ],
        },
        // Tools: engine-wide diagnostics and collaboration
        Menu {
            name: t!("Menu.Tools").into(),
            items: vec![
                MenuItem::action("Profiler", ToggleProfiler),
                MenuItem::action("Mission Control", ToggleConsole),
                MenuItem::action("Type Debugger", ShowTypeDebugger),
                MenuItem::separator(),
                MenuItem::action("Multiplayer", ToggleNetwork),
            ],
        },
        // Window
        Menu {
            name: t!("Menu.Window").into(),
            items: vec![
                MenuItem::action(t!("Menu.Window.NewWindow").to_string(), NewWindow),
                MenuItem::action(t!("Menu.Window.CloseWindow").to_string(), CloseWindow),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.Window.Minimize").to_string(), Minimize),
                MenuItem::action(t!("Menu.Window.Zoom").to_string(), Zoom),
                MenuItem::action(
                    t!("Menu.Window.BringAllToFront").to_string(),
                    BringAllToFront,
                ),
            ],
        },
        // Help
        Menu {
            name: t!("Menu.Help").into(),
            items: vec![
                MenuItem::action(t!("Menu.Help.Documentation").to_string(), ShowDocumentation),
                MenuItem::action(t!("Menu.Help.Shortcuts").to_string(), ShowShortcuts),
                MenuItem::action(t!("Menu.View.CommandPalette").to_string(), CommandPalette),
                MenuItem::separator(),
                MenuItem::action(t!("Menu.Help.ReportBug").to_string(), ReportIssue),
                MenuItem::action(t!("Menu.Help.ViewLogs").to_string(), ViewLogs),
                MenuItem::action(t!("Menu.Help.ReleaseNotes").to_string(), ReleaseNotes),
            ],
        },
    ]
}

/// Initialize the app menus.
///
/// Calls `cx.set_menus` (native macOS menu bar) **and** stores an
/// [`ui::AppMenusCache`] global so that [`ui::AppMenuBar`] can render the
/// in-window menu bar on Windows / Linux where `cx.get_menus()` returns `None`.
pub fn init_app_menus(title: impl Into<SharedString>, cx: &mut App) {
    let title_str: SharedString = title.into();

    // macOS: platform-level native menu bar
    cx.set_menus(build_app_menus(title_str.clone()));

    // Windows / Linux: cache as OwnedMenu for the custom in-window AppMenuBar
    let owned: Vec<gpui::OwnedMenu> = build_app_menus(title_str)
        .into_iter()
        .map(|m| m.owned())
        .collect();
    cx.set_global(ui::AppMenusCache(owned));
}

/// Events emitted by `AppTitleBar` that parent shells can subscribe to.
pub enum AppTitleBarEvent {
    /// User wants to open the multiplayer sessions / friends panel.
    MultiplayerSessionsRequested,
    /// User wants to edit the global Git settings.
    GitSettingsRequested,
}

pub struct AppTitleBar {
    app_menu_bar: Entity<AppMenuBar>,
    locale_picker: Entity<crate::locale_picker::LocalePicker>,
    font_size_selector: Entity<FontSizeSelector>,
    theme_switcher: Entity<ThemeSwitcher>,
    theme_picker: Entity<crate::theme_dropdown::ThemePicker>,
    title: SharedString,
    last_locale: String,
    child: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
    profile_dropdown: Entity<crate::profile_dropdown::ProfileDropdown>,
    _subscriptions: Vec<Subscription>,
    auth_device_code: Option<String>,
    auth_device_verification_url: Option<String>,
    auth_device_notified: bool,
}

impl gpui::EventEmitter<AppTitleBarEvent> for AppTitleBar {}

impl AppTitleBar {
    pub fn new(
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_str = title.into();
        init_app_menus(title_str.clone(), cx);

        let app_menu_bar = new_menu_bar(window, cx);
        let locale_picker = cx.new(|cx| crate::locale_picker::LocalePicker::new(window, cx));
        let font_size_selector = cx.new(|cx| FontSizeSelector::new(window, cx));
        let theme_switcher = cx.new(|cx| ThemeSwitcher::new(cx));
        let theme_picker = cx.new(|cx| crate::theme_dropdown::ThemePicker::new(window, cx));
        let profile_dropdown = cx.new(crate::profile_dropdown::ProfileDropdown::new);

        let subscriptions = vec![cx.subscribe(
            &profile_dropdown,
            |this, _, event: &crate::profile_dropdown::ProfileDropdownEvent, cx| {
                match event {
                    ProfileDropdownEvent::MultiplayerSessionsRequested => {
                        cx.emit(AppTitleBarEvent::MultiplayerSessionsRequested);
                    }
                    ProfileDropdownEvent::SignInRequested => {
                        let Some(client_id) = pulsar_auth::github_client_id_from_env() else {
                            return;
                        };
                        let pd = this.profile_dropdown.clone();
                        let handle = cx.entity().clone();
                        let cid = client_id.clone();
                        cx.spawn(async move |_, cx| {
                            let c_id = cid.clone();
                            let flow = cx
                                .background_executor()
                                .spawn(async move { pulsar_auth::start_device_flow(&c_id) })
                                .await;
                            let flow = match flow {
                                Ok(f) => f,
                                Err(_) => return,
                            };
                            let user_code = flow.user_code.clone();
                            let uri = flow.verification_uri.clone();
                            let _ = open::that(&uri);
                            let _ = cx.update(|cx| {
                                handle.update(cx, |this, cx| {
                                    this.auth_device_code = Some(user_code);
                                    this.auth_device_verification_url = Some(uri.clone());
                                    this.auth_device_notified = false;
                                    cx.notify();
                                });
                            });
                            let c_id2 = cid.to_string();
                            let flow_clone = flow.clone();
                            let token = cx
                                .background_executor()
                                .spawn(async move {
                                    pulsar_auth::wait_for_device_flow_token(&c_id2, &flow_clone)
                                })
                                .await;
                            let token = match token {
                                Ok(t) => t,
                                Err(_) => return,
                            };
                            let token_fetch = token.clone();
                            let profile = cx
                                .background_executor()
                                .spawn(async move { pulsar_auth::fetch_profile(&token_fetch) })
                                .await;
                            let profile = match profile {
                                Ok(p) => p,
                                Err(_) => return,
                            };
                            let _ = pulsar_auth::store_access_token(&token);
                            let _ = pulsar_auth::save_cached_profile(&profile);
                            if let Some(ec) = engine_state::EngineContext::global() {
                                ec.set_auth_profile(profile);
                            }
                            let _ = cx.update(|cx| {
                                pd.update(cx, |d, cx| {
                                    d.ensure_avatar_loaded(cx);
                                    cx.notify();
                                });
                                handle.update(cx, |this, cx| {
                                    this.auth_device_code = None;
                                    this.auth_device_verification_url = None;
                                    cx.notify();
                                });
                            });
                        })
                        .detach();
                    }
                    ProfileDropdownEvent::GitSettingsRequested => {
                        cx.emit(AppTitleBarEvent::GitSettingsRequested);
                    }
                    _ => {}
                }
                cx.notify();
            },
        )];

        Self {
            app_menu_bar,
            locale_picker,
            font_size_selector,
            theme_switcher,
            theme_picker,
            title: title_str,
            last_locale: locale().to_string(),
            child: Rc::new(|_, _| div().into_any_element()),
            profile_dropdown,
            _subscriptions: subscriptions,
            auth_device_code: None,
            auth_device_verification_url: None,
            auth_device_notified: false,
        }
    }

    pub fn child<F, E>(mut self, f: F) -> Self
    where
        E: IntoElement,
        F: Fn(&mut Window, &mut App) -> E + 'static,
    {
        self.child = Rc::new(move |window, cx| f(window, cx).into_any_element());
        self
    }

    fn change_color_mode(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mode = match cx.theme().mode.is_dark() {
            true => ThemeMode::Light,
            false => ThemeMode::Dark,
        };

        Theme::change(mode, None, cx);
    }
}

impl Render for AppTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only rebuild menus if locale changed
        let current_locale = locale().to_string();
        if current_locale != self.last_locale {
            tracing::error!(
                "DEBUG: Locale changed from {} to {}",
                self.last_locale,
                current_locale
            );
            tracing::error!("DEBUG: Test translation Menu.File = {}", t!("Menu.File"));

            // Rebuild menus and app menu bar
            init_app_menus(self.title.clone(), cx);
            self.app_menu_bar = new_menu_bar(window, cx);
            self.last_locale = current_locale;
        }

        let notifications_count = window.notifications(cx).len();

        if self.auth_device_code.is_none() && self.auth_device_notified {
            self.auth_device_notified = false;
            window.close_modal(cx);
        }

        if let Some(ref code) = self.auth_device_code {
            if !self.auth_device_notified {
                self.auth_device_notified = true;
                let url = self
                    .auth_device_verification_url
                    .as_deref()
                    .unwrap_or("https://github.com/login/device")
                    .to_string();
                ui_auth::modal::open_device_code_modal(code, &url, window, cx);
            }
        }

        let dev_popover = cx.new(DevPopover::new);

        TitleBar::new()
            .unified_background(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.app_menu_bar.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_2()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.child.clone()(window, cx))
                    .when(
                        engine_state::EngineContext::global()
                            .map(|ctx| {
                                ctx.store
                                    .get_or_init::<engine_state::DevContext>()
                                    .read()
                                    .is_source_build
                            })
                            .unwrap_or(false),
                        |el| {
                            el.child(
                                ui::popover::Popover::<DevPopover>::new("dev-popover")
                                    .anchor(Corner::TopRight)
                                    .trigger(
                                        Button::new("dev-menu")
                                            .label("Dev")
                                            .icon(IconName::Bug)
                                            .small()
                                            .ghost()
                                            .tooltip("Developer menu and runtime diagnostics"),
                                    )
                                    .content(move |_, _| dev_popover.clone()),
                            )
                        },
                    )
                    .child({
                        let theme_picker = self.theme_picker.clone();
                        ui::popover::Popover::<crate::theme_dropdown::ThemePicker>::new(
                            "theme-picker",
                        )
                        .anchor(Corner::TopRight)
                        .trigger(
                            Button::new("theme-picker-btn")
                                .icon(IconName::Palette)
                                .small()
                                .ghost()
                                .tooltip("Switch theme"),
                        )
                        .content(move |_, _| theme_picker.clone())
                    })
                    .child(
                        Button::new("theme-mode")
                            .map(|this| {
                                if cx.theme().mode.is_dark() {
                                    this.icon(IconName::Sun)
                                } else {
                                    this.icon(IconName::Moon)
                                }
                            })
                            .small()
                            .ghost()
                            .tooltip("Toggle light / dark mode")
                            .on_click(cx.listener(Self::change_color_mode)),
                    )
                    .child(
                        ui::popover::Popover::<crate::locale_picker::LocalePicker>::new(
                            "locale-picker",
                        )
                        .anchor(Corner::TopRight)
                        .trigger(
                            Button::new("locale-picker-btn")
                                .icon(IconName::Globe)
                                .small()
                                .ghost()
                                .tooltip("Change language"),
                        )
                        .content({
                            let picker = self.locale_picker.clone();
                            move |_, _| picker.clone()
                        }),
                    )
                    .child(self.font_size_selector.clone())
                    .child(
                        Button::new("github")
                            .icon(IconName::Git)
                            .small()
                            .ghost()
                            .tooltip("Open the Pulsar GitHub repository")
                            .on_click(|_, _, cx| {
                                cx.open_url("https://github.com/Far-Beyond-Pulsar/Pulsar-Native")
                            }),
                    )
                    .child(
                        div().relative().child(
                            Badge::new().count(notifications_count).max(99).child(
                                Button::new("bell")
                                    .small()
                                    .ghost()
                                    .compact()
                                    .tooltip("Notifications")
                                    .icon(IconName::Bell),
                            ),
                        ),
                    )
                    .child(self.profile_dropdown.clone()),
            )
    }
}

struct FontSizeSelector {
    focus_handle: FocusHandle,
}

impl FontSizeSelector {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }

    fn on_select_font(
        &mut self,
        font_size: &SelectFont,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Theme::global_mut(cx).font_size = px(font_size.0 as f32);
        window.refresh();
    }

    fn on_select_radius(
        &mut self,
        radius: &SelectRadius,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Theme::global_mut(cx).radius = px(radius.0 as f32);
        window.refresh();
    }

    fn on_select_scrollbar_show(
        &mut self,
        show: &SelectScrollbarShow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Theme::global_mut(cx).scrollbar_show = show.0;
        window.refresh();
    }
}

impl Render for FontSizeSelector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();
        let font_size = cx.theme().font_size.as_f32();
        let radius = cx.theme().radius.as_f32();
        let scroll_show = cx.theme().scrollbar_show;

        div()
            .id("font-size-selector")
            .track_focus(&focus_handle)
            .on_action(cx.listener(Self::on_select_font))
            .on_action(cx.listener(Self::on_select_radius))
            .on_action(cx.listener(Self::on_select_scrollbar_show))
            .child(
                Button::new("btn")
                    .small()
                    .ghost()
                    .icon(IconName::Settings2)
                    .tooltip("Appearance & font settings")
                    .popup_menu(move |this, _, _| {
                        this.scrollable()
                            .max_h(px(480.0))
                            .label("Font Size")
                            .menu_with_check("Large", font_size == 18.0, Box::new(SelectFont(18)))
                            .menu_with_check("Medium", font_size == 16.0, Box::new(SelectFont(16)))
                            .menu_with_check(
                                "Small (default)",
                                font_size == 14.0,
                                Box::new(SelectFont(14)),
                            )
                            .separator()
                            .label("Border Radius")
                            .menu_with_check("8px", radius == 8.0, Box::new(SelectRadius(8)))
                            .menu_with_check(
                                "6px (default)",
                                radius == 6.0,
                                Box::new(SelectRadius(6)),
                            )
                            .menu_with_check("4px", radius == 4.0, Box::new(SelectRadius(4)))
                            .menu_with_check("0px", radius == 0.0, Box::new(SelectRadius(0)))
                            .separator()
                            .label("Scrollbar")
                            .menu_with_check(
                                "Scrolling to show",
                                scroll_show == ScrollbarShow::Scrolling,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Scrolling)),
                            )
                            .menu_with_check(
                                "Hover to show",
                                scroll_show == ScrollbarShow::Hover,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Hover)),
                            )
                            .menu_with_check(
                                "Always show",
                                scroll_show == ScrollbarShow::Always,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Always)),
                            )
                    })
                    .anchor(Corner::TopRight),
            )
    }
}

static LOGO_PNG: &[u8] = include_bytes!("../../../../../assets/images/logo_sqrkl.png");

/// The Pulsar logo as a GPUI image, shown in place of the "Pulsar Engine"
/// label on the application menu.
fn decode_logo() -> Option<Arc<gpui::RenderImage>> {
    let rgba = image::load_from_memory(LOGO_PNG).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(gpui::RenderImage::new(smallvec::smallvec![frame])))
}

/// Build the in-window menu bar with the logo applied to the app menu.
fn new_menu_bar(window: &mut Window, cx: &mut App) -> Entity<AppMenuBar> {
    let bar = AppMenuBar::new(window, cx);
    let logo = decode_logo();
    bar.update(cx, |bar, cx| bar.set_logo(logo, cx));
    bar
}

impl AppTitleBar {
    /// The app menu, shown as the Pulsar logo. The bar itself no longer draws
    /// it; the shell places this beside the (taller) header.
    pub fn app_menu_view(&self, cx: &App) -> Option<gpui::AnyView> {
        self.app_menu_bar.read(cx).app_menu_view()
    }
}
