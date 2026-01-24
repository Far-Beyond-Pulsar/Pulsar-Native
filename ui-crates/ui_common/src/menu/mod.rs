use std::rc::Rc;
use rust_i18n::t;

use gpui::{
    actions,
    div,
    prelude::FluentBuilder as _,
    px,
    AnyElement,
    App,
    AppContext,
    ClickEvent,
    Context,
    Corner,
    Entity,
    FocusHandle,
    InteractiveElement as _,
    IntoElement,
    Menu,
    MenuItem,
    MouseButton,
    ParentElement as _,
    Render,
    SharedString,
    Styled as _,
    Subscription,
    Window,
};
use ui::{
    badge::Badge,
    button::{ Button, ButtonVariants as _ },
    locale,
    menu::AppMenuBar,
    popup_menu::PopupMenuExt as _,
    scroll::ScrollbarShow,
    set_locale,
    ActiveTheme as _,
    ContextModal as _,
    IconName,
    PixelsExt,
    Sizable as _,
    Theme,
    ThemeMode,
    TitleBar,
};

use ui::{ themes::ThemeSwitcher, OpenSettings };

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
actions!(menu, [
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
]);

/// Initialize the app menus
pub fn init_app_menus(title: impl Into<SharedString>, cx: &mut App) {
    cx.set_menus(
        vec![
            // Pulsar Menu
            Menu {
                name: title.into(),
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
                    MenuItem::action(t!("Menu.App.QuitApp").to_string(), QuitApp)
                ],
            },
            // File Menu
            Menu {
                name: t!("Menu.File").into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.File.New").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.File.NewFile").to_string(), NewFile),
                            MenuItem::action(t!("Menu.File.NewWindow").to_string(), NewWindow),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.File.NewProject").to_string(), NewProject),
                            MenuItem::action(t!("Menu.File.NewScene").to_string(), NewScene),
                            MenuItem::action(t!("Menu.File.NewScript").to_string(), NewScript),
                            MenuItem::action(t!("Menu.File.NewShader").to_string(), NewShader),
                            MenuItem::action(t!("Menu.File.NewMaterial").to_string(), NewMaterial),
                            MenuItem::action(t!("Menu.File.NewPrefab").to_string(), NewPrefab),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.File.NewBlueprint").to_string(), NewBlueprint),
                            MenuItem::action(t!("Menu.File.NewComponent").to_string(), NewComponent),
                            MenuItem::action(t!("Menu.File.NewSystem").to_string(), NewSystem)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.File.Open").to_string(), OpenFile),
                    MenuItem::action(t!("Menu.File.OpenFolder").to_string(), OpenFolder),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.File.OpenRecent").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.File.RecentProjects").to_string(), OpenRecent),
                            MenuItem::action(t!("Menu.File.RecentFiles").to_string(), OpenRecentFiles),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.File.ClearRecent").to_string(), ClearRecent)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.File.Save").to_string(), SaveFile),
                    MenuItem::action(t!("Menu.File.SaveAs").to_string(), SaveAs),
                    MenuItem::action(t!("Menu.File.SaveAll").to_string(), SaveAll),
                    MenuItem::action(t!("Menu.File.SaveWorkspace").to_string(), SaveWorkspace),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.File.Import").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.File.ImportAsset").to_string(), ImportAsset),
                            MenuItem::action(t!("Menu.File.ImportModel").to_string(), ImportModel),
                            MenuItem::action(t!("Menu.File.ImportTexture").to_string(), ImportTexture),
                            MenuItem::action(t!("Menu.File.ImportAudio").to_string(), ImportAudio),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.File.BatchImport").to_string(), BatchImport),
                            MenuItem::action(t!("Menu.File.ImportFromUnity").to_string(), ImportFromUnity),
                            MenuItem::action(t!("Menu.File.ImportFromUnreal").to_string(), ImportFromUnreal),
                            MenuItem::action(t!("Menu.File.ImportFromGodot").to_string(), ImportFromGodot)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.File.Export").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.File.ExportBuild").to_string(), ExportBuild),
                            MenuItem::action(t!("Menu.File.ExportScene").to_string(), ExportScene),
                            MenuItem::action(t!("Menu.File.ExportSelected").to_string(), ExportSelected),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.File.ExportWindows").to_string(), ExportWindows),
                            MenuItem::action(t!("Menu.File.ExportLinux").to_string(), ExportLinux),
                            MenuItem::action(t!("Menu.File.ExportMacOS").to_string(), ExportMacOS),
                            MenuItem::action(t!("Menu.File.ExportWeb").to_string(), ExportWeb),
                            MenuItem::action(t!("Menu.File.ExportAndroid").to_string(), ExportAndroid),
                            MenuItem::action(t!("Menu.File.ExportIOS").to_string(), ExportIOS)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.File.RevertFile").to_string(), RevertFile),
                    MenuItem::action(t!("Menu.File.CloseFile").to_string(), CloseFile),
                    MenuItem::action(t!("Menu.File.CloseFolder").to_string(), CloseFolder),
                    MenuItem::action(t!("Menu.File.CloseAll").to_string(), CloseAll),
                    MenuItem::action(t!("Menu.File.CloseOthers").to_string(), CloseOthers)
                ],
            },
            // Edit Menu
            Menu {
                name: t!("Menu.Edit").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Edit.Undo").to_string(), Undo),
                    MenuItem::action(t!("Menu.Edit.Redo").to_string(), Redo),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Edit.Cut").to_string(), Cut),
                    MenuItem::action(t!("Menu.Edit.Copy").to_string(), Copy),
                    MenuItem::action(t!("Menu.Edit.Paste").to_string(), Paste),
                    MenuItem::action(t!("Menu.Edit.Delete").to_string(), Delete),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Edit.SelectAll").to_string(), SelectAll),
                    MenuItem::action(t!("Menu.Edit.SelectNone").to_string(), SelectNone),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Edit.Find").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Edit.Find").to_string(), Find),
                            MenuItem::action(t!("Menu.Edit.FindNext").to_string(), FindNext),
                            MenuItem::action(t!("Menu.Edit.FindPrevious").to_string(), FindPrevious),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.Replace").to_string(), FindReplace),
                            MenuItem::action(t!("Menu.Edit.ReplaceNext").to_string(), ReplaceNext),
                            MenuItem::action(t!("Menu.Edit.ReplaceAll").to_string(), ReplaceAll),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.FindInFiles").to_string(), FindInFiles),
                            MenuItem::action(t!("Menu.Edit.ReplaceInFiles").to_string(), ReplaceInFiles),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.FindUsages").to_string(), FindUsages),
                            MenuItem::action(t!("Menu.Edit.FindImplementations").to_string(), FindImplementations)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Edit.CodeActions").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Edit.FormatDocument").to_string(), FormatDocument),
                            MenuItem::action(t!("Menu.Edit.FormatSelection").to_string(), FormatSelection),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.CommentLine").to_string(), CommentLine),
                            MenuItem::action(t!("Menu.Edit.UncommentLine").to_string(), UncommentLine),
                            MenuItem::action(t!("Menu.Edit.ToggleComment").to_string(), ToggleComment),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.Fold").to_string(), Fold),
                            MenuItem::action(t!("Menu.Edit.Unfold").to_string(), Unfold),
                            MenuItem::action(t!("Menu.Edit.FoldAll").to_string(), FoldAll),
                            MenuItem::action(t!("Menu.Edit.UnfoldAll").to_string(), UnfoldAll),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Edit.SortLines").to_string(), SortLines),
                            MenuItem::action(t!("Menu.Edit.RemoveDuplicates").to_string(), RemoveDuplicates),
                            MenuItem::action(t!("Menu.Edit.TrimWhitespace").to_string(), TrimWhitespace)
                        ],
                    })
                ],
            },
            // Selection Menu
            Menu {
                name: t!("Menu.Selection").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Selection.SelectLine").to_string(), SelectLine),
                    MenuItem::action(t!("Menu.Selection.SelectWord").to_string(), SelectWord),
                    MenuItem::action(t!("Menu.Selection.SelectScope").to_string(), SelectScope),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Selection.ExpandSelection").to_string(), ExpandSelection),
                    MenuItem::action(t!("Menu.Selection.ShrinkSelection").to_string(), ShrinkSelection),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Selection.AddCursorAbove").to_string(), AddCursorAbove),
                    MenuItem::action(t!("Menu.Selection.AddCursorBelow").to_string(), AddCursorBelow),
                    MenuItem::action(t!("Menu.Selection.AddCursorLineEnds").to_string(), AddCursorLineEnds),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Selection.SelectAllOccurrences").to_string(), SelectAllOccurrences),
                    MenuItem::action(t!("Menu.Selection.SelectNextOccurrence").to_string(), SelectNextOccurrence),
                    MenuItem::action(t!("Menu.Selection.SkipOccurrence").to_string(), SkipOccurrence)
                ],
            },
            // View Menu
            Menu {
                name: t!("Menu.View").into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.View.Panels").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.View.Explorer").to_string(), ToggleExplorer),
                            MenuItem::action(t!("Menu.View.Hierarchy").to_string(), ToggleHierarchy),
                            MenuItem::action(t!("Menu.View.Inspector"), ToggleInspector),
                            MenuItem::action(t!("Menu.View.AssetBrowser").to_string(), ToggleAssetBrowser),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.View.Console"), ToggleConsole),
                            MenuItem::action(t!("Menu.View.Output").to_string(), ToggleOutput),
                            MenuItem::action(t!("Menu.View.Problems").to_string(), ToggleProblems),
                            MenuItem::action(t!("Menu.View.Debug").to_string(), ToggleDebug),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.View.Profiler"), ToggleProfiler),
                            MenuItem::action(t!("Menu.View.MemoryAnalyzer").to_string(), ToggleMemoryAnalyzer),
                            MenuItem::action(t!("Menu.View.Network").to_string(), ToggleNetwork)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.View.Layout").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.View.SplitHorizontal").to_string(), SplitHorizontal),
                            MenuItem::action(t!("Menu.View.SplitVertical").to_string(), SplitVertical),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.View.SingleColumn").to_string(), SingleColumn),
                            MenuItem::action(t!("Menu.View.TwoColumns").to_string(), TwoColumns),
                            MenuItem::action(t!("Menu.View.ThreeColumns").to_string(), ThreeColumns),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.View.ResetLayout").to_string(), ResetLayout),
                            MenuItem::action(t!("Menu.View.SaveLayout").to_string(), SaveLayout)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.View.CommandPalette"), CommandPalette),
                    MenuItem::action(t!("Menu.View.QuickOpen").to_string(), QuickOpen),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Zoom".into(),
                        items: vec![
                            MenuItem::action(t!("Menu.View.ZoomIn"), ZoomIn),
                            MenuItem::action(t!("Menu.View.ZoomOut"), ZoomOut),
                            MenuItem::action(t!("Menu.View.ResetZoom"), ResetZoom)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.View.ToggleMinimap"), ToggleMinimap),
                    MenuItem::action(t!("Menu.View.ToggleLineNumbers"), ToggleLineNumbers),
                    MenuItem::action(t!("Menu.View.ToggleBreadcrumbs"), ToggleBreadcrumbs),
                    MenuItem::action(t!("Menu.View.ToggleWhitespace"), ToggleWhitespace),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.View.ToggleFullscreen"), ToggleFullscreen),
                    MenuItem::action(t!("Menu.View.ZenMode"), ToggleZenMode)
                ],
            },
            // Go Menu
            Menu {
                name: t!("Menu.Go").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Go.GoToFile").to_string(), GoToFile),
                    MenuItem::action(t!("Menu.Go.GoToSymbol").to_string(), GoToSymbol),
                    MenuItem::action(t!("Menu.Go.GoToLine").to_string(), GoToLine),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Go.GoToDefinition").to_string(), GoToDefinition),
                    MenuItem::action(t!("Menu.Go.GoToTypeDefinition").to_string(), GoToTypeDefinition),
                    MenuItem::action(t!("Menu.Go.GoToImplementation").to_string(), GoToImplementation),
                    MenuItem::action(t!("Menu.Go.GoToReferences").to_string(), GoToReferences),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Go.GoBack").to_string(), GoBack),
                    MenuItem::action(t!("Menu.Go.GoForward").to_string(), GoForward),
                    MenuItem::action(t!("Menu.Go.GoToLastEdit").to_string(), GoToLastEdit),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Go.NextProblem").to_string(), NextProblem),
                    MenuItem::action(t!("Menu.Go.PreviousProblem").to_string(), PreviousProblem)
                ],
            },
            // Project Menu
            Menu {
                name: t!("Menu.Project").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Project.ProjectSettings").to_string(), ProjectSettings),
                    MenuItem::action(t!("Menu.Project.BuildSettings").to_string(), BuildSettings),
                    MenuItem::action(t!("Menu.Project.PackageSettings").to_string(), PackageSettings),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Project.Dependencies").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Project.AddDependency").to_string(), AddDependency),
                            MenuItem::action(t!("Menu.Project.UpdateDependencies").to_string(), UpdateDependencies),
                            MenuItem::action(t!("Menu.Project.RemoveUnused").to_string(), RemoveUnusedDeps),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Project.OpenCargoToml").to_string(), OpenCargoToml)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Project.GenerateDocs").to_string(), GenerateDocs),
                    MenuItem::action(t!("Menu.Project.RunCargoCheck").to_string(), RunCargoCheck),
                    MenuItem::action(t!("Menu.Project.RunClippy").to_string(), RunClippy),
                    MenuItem::action(t!("Menu.Project.FormatProject").to_string(), FormatProject)
                ],
            },
            // Build Menu
            Menu {
                name: t!("Menu.Build").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Build.BuildProject").to_string(), Build),
                    MenuItem::action(t!("Menu.Build.BuildAndRun").to_string(), BuildAndRun),
                    MenuItem::action(t!("Menu.Build.BuildRelease").to_string(), BuildRelease),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Build.Rebuild").to_string(), Rebuild),
                    MenuItem::action(t!("Menu.Build.Clean").to_string(), Clean),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Build.BuildConfiguration").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Build.BuildDebug").to_string(), BuildDebug),
                            MenuItem::action(t!("Menu.Build.BuildRelease").to_string(), BuildRelease),
                            MenuItem::action(t!("Menu.Build.BuildReleaseDebug").to_string(), BuildReleaseDebug),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Build.BuildProfile").to_string(), BuildProfile)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Build.CancelBuild").to_string(), CancelBuild),
                    MenuItem::action(t!("Menu.Build.ShowBuildOutput").to_string(), ShowBuildOutput)
                ],
            },
            // Run Menu
            Menu {
                name: t!("Menu.Run").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Run.RunProject").to_string(), RunProject),
                    MenuItem::action(t!("Menu.Run.RunWithoutDebug").to_string(), RunWithoutDebug),
                    MenuItem::action(t!("Menu.Run.Stop").to_string(), StopExecution),
                    MenuItem::action(t!("Menu.Run.Restart").to_string(), RestartExecution),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Run.DebugProject").to_string(), DebugProject),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Run.Debugging").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Run.StartDebug").to_string(), StartDebug),
                            MenuItem::action(t!("Menu.Run.StopDebug").to_string(), StopDebug),
                            MenuItem::action(t!("Menu.Run.RestartDebug").to_string(), RestartDebug),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Run.StepOver").to_string(), StepOver),
                            MenuItem::action(t!("Menu.Run.StepInto").to_string(), StepInto),
                            MenuItem::action(t!("Menu.Run.StepOut").to_string(), StepOut),
                            MenuItem::action(t!("Menu.Run.Continue").to_string(), Continue),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Run.ToggleBreakpoint").to_string(), ToggleBreakpoint),
                            MenuItem::action(t!("Menu.Run.DisableBreakpoints").to_string(), DisableBreakpoints),
                            MenuItem::action(t!("Menu.Run.RemoveBreakpoints").to_string(), RemoveBreakpoints)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Run.Testing").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Run.RunTests").to_string(), RunTests),
                            MenuItem::action(t!("Menu.Run.RunFailedTests").to_string(), RunFailedTests),
                            MenuItem::action(t!("Menu.Run.RunFileTests").to_string(), RunFileTests),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Run.DebugTest").to_string(), DebugTest),
                            MenuItem::action(t!("Menu.Run.RunCoverage").to_string(), RunCoverage)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Run.Performance").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Run.RunBenchmarks").to_string(), RunBenchmarks),
                            MenuItem::action(t!("Menu.Run.CompareBenchmarks").to_string(), CompareBenchmarks),
                            MenuItem::action(t!("Menu.Run.ProfileBuild").to_string(), ProfileBuild)
                        ],
                    })
                ],
            },
            // Engine Menu
            Menu {
                name: t!("Menu.GameEngine").into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.GameEngine.Scene").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.GameEngine.NewScene").to_string(), NewScene),
                            MenuItem::action(t!("Menu.GameEngine.OpenScene").to_string(), OpenScene),
                            MenuItem::action(t!("Menu.GameEngine.SaveScene").to_string(), SaveScene),
                            MenuItem::action(t!("Menu.GameEngine.SaveSceneAs").to_string(), SaveSceneAs),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.GameEngine.PlayScene").to_string(), PlayScene),
                            MenuItem::action(t!("Menu.GameEngine.PauseScene").to_string(), PauseScene),
                            MenuItem::action(t!("Menu.GameEngine.StopScene").to_string(), StopScene),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.GameEngine.SceneSettings").to_string(), SceneSettings)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.GameEngine.GameObject").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.GameEngine.CreateEmpty").to_string(), CreateEmpty),
                            MenuItem::separator(),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.3DObjects").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateCube").to_string(), CreateCube),
                                    MenuItem::action(t!("Menu.GameEngine.CreateSphere").to_string(), CreateSphere),
                                    MenuItem::action(t!("Menu.GameEngine.CreateCapsule").to_string(), CreateCapsule),
                                    MenuItem::action(t!("Menu.GameEngine.CreateCylinder").to_string(), CreateCylinder),
                                    MenuItem::action(t!("Menu.GameEngine.CreatePlane").to_string(), CreatePlane),
                                    MenuItem::separator(),
                                    MenuItem::action(t!("Menu.GameEngine.CreateTerrain").to_string(), CreateTerrain)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.2DObjects").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateSprite").to_string(), CreateSprite),
                                    MenuItem::action(t!("Menu.GameEngine.CreateTilemap").to_string(), CreateTilemap),
                                    MenuItem::action(t!("Menu.GameEngine.CreateParticles2D").to_string(), CreateParticles2D)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.Light").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateDirectionalLight").to_string(), CreateDirectionalLight),
                                    MenuItem::action(t!("Menu.GameEngine.CreatePointLight").to_string(), CreatePointLight),
                                    MenuItem::action(t!("Menu.GameEngine.CreateSpotLight").to_string(), CreateSpotLight),
                                    MenuItem::action(t!("Menu.GameEngine.CreateAreaLight").to_string(), CreateAreaLight)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.Audio").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateAudioSource").to_string(), CreateAudioSource),
                                    MenuItem::action(t!("Menu.GameEngine.CreateAudioListener").to_string(), CreateAudioListener)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.Effects").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateParticleSystem").to_string(), CreateParticleSystem),
                                    MenuItem::action(t!("Menu.GameEngine.CreateVFX").to_string(), CreateVFX),
                                    MenuItem::action(t!("Menu.GameEngine.CreatePostProcessing").to_string(), CreatePostProcessing)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.Camera").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateCamera").to_string(), CreateCamera),
                                    MenuItem::action(t!("Menu.GameEngine.CreateOrthoCamera").to_string(), CreateOrthoCamera)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: t!("Menu.GameEngine.UI").into(),
                                items: vec![
                                    MenuItem::action(t!("Menu.GameEngine.CreateCanvas").to_string(), CreateCanvas),
                                    MenuItem::action(t!("Menu.GameEngine.CreatePanel").to_string(), CreatePanel),
                                    MenuItem::action(t!("Menu.GameEngine.CreateButton").to_string(), CreateButton),
                                    MenuItem::action(t!("Menu.GameEngine.CreateText").to_string(), CreateText),
                                    MenuItem::action(t!("Menu.GameEngine.CreateImage").to_string(), CreateImage),
                                    MenuItem::action(t!("Menu.GameEngine.CreateSlider").to_string(), CreateSlider),
                                    MenuItem::action(t!("Menu.GameEngine.CreateInputField").to_string(), CreateInputField)
                                ],
                            })
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.GameEngine.Physics").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.GameEngine.AddRigidbody").to_string(), AddRigidbody),
                            MenuItem::action(t!("Menu.GameEngine.AddCollider").to_string(), AddCollider),
                            MenuItem::action(t!("Menu.GameEngine.AddCharacterController").to_string(), AddCharacterController),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.GameEngine.PhysicsSettings").to_string(), PhysicsSettings),
                            MenuItem::action(t!("Menu.GameEngine.CollisionMatrix").to_string(), CollisionMatrix)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.GameEngine.Rendering").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.GameEngine.RenderSettings").to_string(), RenderSettings),
                            MenuItem::action(t!("Menu.GameEngine.LightingSettings").to_string(), LightingSettings),
                            MenuItem::action(t!("Menu.GameEngine.QualitySettings").to_string(), QualitySettings),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.GameEngine.BakeLighting").to_string(), BakeLighting),
                            MenuItem::action(t!("Menu.GameEngine.ClearBakedData").to_string(), ClearBakedData),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.GameEngine.FrameDebugger").to_string(), FrameDebugger),
                            MenuItem::action(t!("Menu.GameEngine.EngineProfiler").to_string(), EngineProfiler)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.GameEngine.PackageManager").to_string(), PackageManager),
                    MenuItem::action(t!("Menu.GameEngine.AssetStore").to_string(), AssetStore)
                ],
            },
            // Assets Menu
            Menu {
                name: t!("Menu.Assets").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Assets.CreateAsset").to_string(), CreateAsset),
                    MenuItem::action(t!("Menu.Assets.ImportAsset").to_string(), ImportAsset),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Assets.Create").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Assets.CreateMaterial").to_string(), CreateMaterial),
                            MenuItem::action(t!("Menu.Assets.CreateShader").to_string(), CreateShader),
                            MenuItem::action(t!("Menu.Assets.CreateTexture").to_string(), CreateTexture),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Assets.CreateAnimClip").to_string(), CreateAnimClip),
                            MenuItem::action(t!("Menu.Assets.CreateAnimController").to_string(), CreateAnimController),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Assets.CreateAudioMixer").to_string(), CreateAudioMixer),
                            MenuItem::action(t!("Menu.Assets.CreateRenderTexture").to_string(), CreateRenderTexture),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Assets.CreatePrefab").to_string(), CreatePrefab),
                            MenuItem::action(t!("Menu.Assets.CreateScriptableObject").to_string(), CreateScriptableObject)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Assets.Refresh").to_string(), RefreshAssets),
                    MenuItem::action(t!("Menu.Assets.Reimport").to_string(), ReimportAssets),
                    MenuItem::action(t!("Menu.Assets.ReimportAll").to_string(), ReimportAllAssets),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Assets.FindReferences").to_string(), FindAssetReferences),
                    MenuItem::action(t!("Menu.Assets.FindMissingReferences").to_string(), FindMissingReferences)
                ],
            },
            // Tools Menu
            Menu {
                name: t!("Menu.Tools").into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Tools.Rust").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Tools.CargoCommands").to_string(), CargoCommands),
                            MenuItem::action(t!("Menu.Tools.GenerateRustAnalyzer").to_string(), GenerateRustAnalyzer),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Tools.ExpandMacro").to_string(), ExpandMacro),
                            MenuItem::action(t!("Menu.Tools.ShowSyntaxTree").to_string(), ShowSyntaxTree),
                            MenuItem::action(t!("Menu.Tools.ShowHIR").to_string(), ShowHIR),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Tools.InlineVariable").to_string(), InlineVariable),
                            MenuItem::action(t!("Menu.Tools.ExtractFunction").to_string(), ExtractFunction),
                            MenuItem::action(t!("Menu.Tools.ExtractVariable").to_string(), ExtractVariable)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Tools.Shaders").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Tools.ShaderEditor").to_string(), ShaderEditor),
                            MenuItem::action(t!("Menu.Tools.CompileShader").to_string(), CompileShader),
                            MenuItem::action(t!("Menu.Tools.ShaderVariants").to_string(), ShaderVariants),
                            MenuItem::separator(),
                            MenuItem::action(t!("Menu.Tools.SPIRVDisassembly").to_string(), SPIRVDisassembly)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: t!("Menu.Tools.Animation").into(),
                        items: vec![
                            MenuItem::action(t!("Menu.Tools.AnimationWindow").to_string(), AnimationWindow),
                            MenuItem::action(t!("Menu.Tools.AnimatorWindow").to_string(), AnimatorWindow),
                            MenuItem::action(t!("Menu.Tools.Timeline").to_string(), Timeline)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Tools.MaterialEditor").to_string(), MaterialEditor),
                    MenuItem::action(t!("Menu.Tools.TerrainTools").to_string(), TerrainTools),
                    MenuItem::action(t!("Menu.Tools.ParticleEditor").to_string(), ParticleEditor),
                    MenuItem::action(t!("Menu.Tools.AudioMixerWindow").to_string(), AudioMixerWindow),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Tools.VersionControl").to_string(), VersionControl),
                    MenuItem::action(t!("Menu.Tools.TaskManager").to_string(), TaskManager),
                    MenuItem::action(t!("Menu.Tools.Extensions").to_string(), Extensions)
                ],
            },
            // Window Menu
            Menu {
                name: t!("Menu.Window").into(),
                items: vec![
                    MenuItem::action(t!("Menu.Window.Minimize").to_string(), Minimize),
                    MenuItem::action(t!("Menu.Window.Zoom").to_string(), Zoom),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Window.BringAllToFront").to_string(), BringAllToFront),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Window.NewWindow").to_string(), NewWindow),
                    MenuItem::action(t!("Menu.Window.CloseWindow").to_string(), CloseWindow)
                ],
            },
            // Help Menu
            Menu {
                name: t!("Menu.Help").into(),
                items: vec![
                    MenuItem::action(&t!("Menu.Help.Documentation").to_string(), ShowDocumentation),
                    MenuItem::action(&t!("Menu.Help.API").to_string(), ShowAPIReference),
                    MenuItem::action(&t!("Menu.Help.Tutorials").to_string(), ShowTutorials),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Help.Shortcuts").to_string(), ShowShortcuts),
                    MenuItem::action(&t!("Menu.View.CommandPalette").to_string(), CommandPalette),
                    MenuItem::separator(),
                    MenuItem::action(&t!("Menu.Help.ReportBug").to_string(), ReportIssue),
                    MenuItem::action(t!("Menu.Help.ViewLogs").to_string(), ViewLogs),
                    MenuItem::separator(),
                    MenuItem::action(t!("Menu.Help.CheckUpdates").to_string(), CheckUpdates),
                    MenuItem::action(t!("Menu.Help.ReleaseNotes").to_string(), ReleaseNotes),
                    MenuItem::separator(),
                    MenuItem::action(&t!("Menu.Help.About").to_string(), AboutApp)
                ],
            }
        ]
    );
}

pub struct AppTitleBar {
    app_menu_bar: Entity<AppMenuBar>,
    locale_selector: Entity<LocaleSelector>,
    font_size_selector: Entity<FontSizeSelector>,
    theme_switcher: Entity<ThemeSwitcher>,
    title: SharedString,
    last_locale: String,
    child: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
    _subscriptions: Vec<Subscription>,
}

impl AppTitleBar {
    pub fn new(
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Self {
        let title_str = title.into();
        init_app_menus(title_str.clone(), cx);

        let app_menu_bar = AppMenuBar::new(window, cx);
        let locale_selector = cx.new(|cx| LocaleSelector::new(window, cx));
        let font_size_selector = cx.new(|cx| FontSizeSelector::new(window, cx));
        let theme_switcher = cx.new(|cx| ThemeSwitcher::new(cx));
        
        // Subscribe to locale changes
        let subscriptions = vec![
            cx.subscribe(&locale_selector, move |_this: &mut Self, _, _event: &SelectLocale, cx| {
                // Just notify to trigger re-render
                cx.notify();
            })
        ];

        Self {
            app_menu_bar,
            locale_selector,
            font_size_selector,
            theme_switcher,
            title: title_str,
            last_locale: locale().to_string(),
            child: Rc::new(|_, _| div().into_any_element()),
            _subscriptions: subscriptions,
        }
    }

    pub fn child<F, E>(mut self, f: F) -> Self
        where E: IntoElement, F: Fn(&mut Window, &mut App) -> E + 'static
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

// TODO: (From @tristanpoland) Near as I can tell this tracing::info! call is never executed. Look into this when debugging the titlebar
impl Render for AppTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only rebuild menus if locale changed
        let current_locale = locale().to_string();
        if current_locale != self.last_locale {
            eprintln!("DEBUG: Locale changed from {} to {}", self.last_locale, current_locale);
            eprintln!("DEBUG: Test translation Menu.File = {}", t!("Menu.File"));
            
            // Rebuild menus and app menu bar
            init_app_menus(self.title.clone(), cx);
            self.app_menu_bar = AppMenuBar::new(window, cx);
            self.last_locale = current_locale;
        }
        
        let notifications_count = window.notifications(cx).len();

        TitleBar::new()
            // left side with app menu bar
            .child(
                div()
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.app_menu_bar.clone())
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
                    .child(self.theme_switcher.clone())
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
                            .on_click(cx.listener(Self::change_color_mode))
                    )
                    .child(self.locale_selector.clone())
                    .child(self.font_size_selector.clone())
                    .child(
                        Button::new("github")
                            .icon(IconName::GitHub)
                            .small()
                            .ghost()
                            .on_click(|_, _, cx| {
                                cx.open_url("https://github.com/Far-Beyond-Pulsar/Pulsar-Native")
                            })
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                Badge::new()
                                    .count(notifications_count)
                                    .max(99)
                                    .child(
                                        Button::new("bell")
                                            .small()
                                            .ghost()
                                            .compact()
                                            .icon(IconName::Bell)
                                    )
                            )
                    )
            )
    }
}

struct LocaleSelector {
    focus_handle: FocusHandle,
}

impl gpui::EventEmitter<SelectLocale> for LocaleSelector {}

impl LocaleSelector {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }

    fn on_select_locale(
        &mut self,
        locale: &SelectLocale,
        window: &mut Window,
        cx: &mut Context<Self>
    ) {
        // Set locale globally - this affects ALL crates using rust_i18n
        set_locale(&locale.0);
        
        // Also set in ui_level_editor if it has its own translations
        // (Level editor may have its own separate translation context)
        
        // Emit event so AppTitleBar can rebuild menus
        cx.emit(locale.clone());
        
        window.refresh();
    }
}

impl Render for LocaleSelector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();
        let current_locale = locale().to_string();

        div()
            .id("locale-selector")
            .track_focus(&focus_handle)
            .on_action(cx.listener(Self::on_select_locale))
            .child(
                Button::new("btn")
                    .small()
                    .ghost()
                    .icon(IconName::Globe)
                    .popup_menu(move |menu, _, _| {
                        // Add all available locales
                        menu
                            .menu_with_check(
                                "English",
                                current_locale == "en",
                                Box::new(SelectLocale("en".into()))
                            )
                            .menu_with_check(
                                "简体中文 (Simplified Chinese)",
                                current_locale == "zh-CN",
                                Box::new(SelectLocale("zh-CN".into()))
                            )
                            .menu_with_check(
                                "繁體中文 (Traditional Chinese)",
                                current_locale == "zh-HK",
                                Box::new(SelectLocale("zh-HK".into()))
                            )
                            .menu_with_check(
                                "Русский (Russian)",
                                current_locale == "ru",
                                Box::new(SelectLocale("ru".into()))
                            )
                            .menu_with_check(
                                "Italiano (Italian)",
                                current_locale == "it",
                                Box::new(SelectLocale("it".into()))
                            )
                            .menu_with_check(
                                "Português (Portuguese)",
                                current_locale == "pt-BR",
                                Box::new(SelectLocale("pt-BR".into()))
                            )
                            .menu_with_check(
                                "Lolcat",
                                current_locale == "lol",
                                Box::new(SelectLocale("lol".into()))
                            )
                    })
                    .anchor(Corner::TopRight)
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
        cx: &mut Context<Self>
    ) {
        Theme::global_mut(cx).font_size = px(font_size.0 as f32);
        window.refresh();
    }

    fn on_select_radius(
        &mut self,
        radius: &SelectRadius,
        window: &mut Window,
        cx: &mut Context<Self>
    ) {
        Theme::global_mut(cx).radius = px(radius.0 as f32);
        window.refresh();
    }

    fn on_select_scrollbar_show(
        &mut self,
        show: &SelectScrollbarShow,
        window: &mut Window,
        cx: &mut Context<Self>
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
                    .popup_menu(move |this, _, _| {
                        this.scrollable()
                            .max_h(px(480.0))
                            .label("Font Size")
                            .menu_with_check("Large", font_size == 18.0, Box::new(SelectFont(18)))
                            .menu_with_check(
                                "Medium (default)",
                                font_size == 16.0,
                                Box::new(SelectFont(16))
                            )
                            .menu_with_check("Small", font_size == 14.0, Box::new(SelectFont(14)))
                            .separator()
                            .label("Border Radius")
                            .menu_with_check("8px", radius == 8.0, Box::new(SelectRadius(8)))
                            .menu_with_check(
                                "6px (default)",
                                radius == 6.0,
                                Box::new(SelectRadius(6))
                            )
                            .menu_with_check("4px", radius == 4.0, Box::new(SelectRadius(4)))
                            .menu_with_check("0px", radius == 0.0, Box::new(SelectRadius(0)))
                            .separator()
                            .label("Scrollbar")
                            .menu_with_check(
                                "Scrolling to show",
                                scroll_show == ScrollbarShow::Scrolling,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Scrolling))
                            )
                            .menu_with_check(
                                "Hover to show",
                                scroll_show == ScrollbarShow::Hover,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Hover))
                            )
                            .menu_with_check(
                                "Always show",
                                scroll_show == ScrollbarShow::Always,
                                Box::new(SelectScrollbarShow(ScrollbarShow::Always))
                            )
                    })
                    .anchor(Corner::TopRight)
            )
    }
}


