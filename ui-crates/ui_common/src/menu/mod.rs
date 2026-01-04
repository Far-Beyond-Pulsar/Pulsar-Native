use std::rc::Rc;

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
                    MenuItem::action("About Pulsar Engine", AboutApp),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates", CheckUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Preferences", Preferences),
                    MenuItem::action("Settings", Settings),
                    MenuItem::separator(),
                    MenuItem::action("Hide Pulsar", Hide),
                    MenuItem::action("Hide Others", HideOthers),
                    MenuItem::action("Show All", ShowAll),
                    MenuItem::separator(),
                    MenuItem::action("Quit Pulsar", QuitApp)
                ],
            },
            // File Menu
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: "New".into(),
                        items: vec![
                            MenuItem::action("New File", NewFile),
                            MenuItem::action("New Window", NewWindow),
                            MenuItem::separator(),
                            MenuItem::action("New Project", NewProject),
                            MenuItem::action("New Scene", NewScene),
                            MenuItem::action("New Script", NewScript),
                            MenuItem::action("New Shader", NewShader),
                            MenuItem::action("New Material", NewMaterial),
                            MenuItem::action("New Prefab", NewPrefab),
                            MenuItem::separator(),
                            MenuItem::action("New Blueprint", NewBlueprint),
                            MenuItem::action("New Component", NewComponent),
                            MenuItem::action("New System", NewSystem)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Open", OpenFile),
                    MenuItem::action("Open Folder", OpenFolder),
                    MenuItem::Submenu(Menu {
                        name: "Open Recent".into(),
                        items: vec![
                            MenuItem::action("Recent Projects", OpenRecent),
                            MenuItem::action("Recent Files", OpenRecentFiles),
                            MenuItem::separator(),
                            MenuItem::action("Clear Recent", ClearRecent)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Save", SaveFile),
                    MenuItem::action("Save As...", SaveAs),
                    MenuItem::action("Save All", SaveAll),
                    MenuItem::action("Save Workspace", SaveWorkspace),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Import".into(),
                        items: vec![
                            MenuItem::action("Import Asset", ImportAsset),
                            MenuItem::action("Import Model (FBX/GLTF)", ImportModel),
                            MenuItem::action("Import Texture", ImportTexture),
                            MenuItem::action("Import Audio", ImportAudio),
                            MenuItem::separator(),
                            MenuItem::action("Batch Import", BatchImport),
                            MenuItem::action("Import from Unity", ImportFromUnity),
                            MenuItem::action("Import from Unreal", ImportFromUnreal),
                            MenuItem::action("Import from Godot", ImportFromGodot)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: "Export".into(),
                        items: vec![
                            MenuItem::action("Export Build", ExportBuild),
                            MenuItem::action("Export Scene", ExportScene),
                            MenuItem::action("Export Selected", ExportSelected),
                            MenuItem::separator(),
                            MenuItem::action("Export for Windows", ExportWindows),
                            MenuItem::action("Export for Linux", ExportLinux),
                            MenuItem::action("Export for macOS", ExportMacOS),
                            MenuItem::action("Export for Web (WASM)", ExportWeb),
                            MenuItem::action("Export for Android", ExportAndroid),
                            MenuItem::action("Export for iOS", ExportIOS)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Revert File", RevertFile),
                    MenuItem::action("Close File", CloseFile),
                    MenuItem::action("Close Folder", CloseFolder),
                    MenuItem::action("Close All", CloseAll),
                    MenuItem::action("Close Others", CloseOthers)
                ],
            },
            // Edit Menu
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Undo", Undo),
                    MenuItem::action("Redo", Redo),
                    MenuItem::separator(),
                    MenuItem::action("Cut", Cut),
                    MenuItem::action("Copy", Copy),
                    MenuItem::action("Paste", Paste),
                    MenuItem::action("Delete", Delete),
                    MenuItem::separator(),
                    MenuItem::action("Select All", SelectAll),
                    MenuItem::action("Select None", SelectNone),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Find".into(),
                        items: vec![
                            MenuItem::action("Find", Find),
                            MenuItem::action("Find Next", FindNext),
                            MenuItem::action("Find Previous", FindPrevious),
                            MenuItem::separator(),
                            MenuItem::action("Replace", FindReplace),
                            MenuItem::action("Replace Next", ReplaceNext),
                            MenuItem::action("Replace All", ReplaceAll),
                            MenuItem::separator(),
                            MenuItem::action("Find in Files", FindInFiles),
                            MenuItem::action("Replace in Files", ReplaceInFiles),
                            MenuItem::separator(),
                            MenuItem::action("Find Usages", FindUsages),
                            MenuItem::action("Find Implementations", FindImplementations)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Code".into(),
                        items: vec![
                            MenuItem::action("Format Document", FormatDocument),
                            MenuItem::action("Format Selection", FormatSelection),
                            MenuItem::separator(),
                            MenuItem::action("Comment Line", CommentLine),
                            MenuItem::action("Uncomment Line", UncommentLine),
                            MenuItem::action("Toggle Comment", ToggleComment),
                            MenuItem::separator(),
                            MenuItem::action("Fold", Fold),
                            MenuItem::action("Unfold", Unfold),
                            MenuItem::action("Fold All", FoldAll),
                            MenuItem::action("Unfold All", UnfoldAll),
                            MenuItem::separator(),
                            MenuItem::action("Sort Lines", SortLines),
                            MenuItem::action("Remove Duplicates", RemoveDuplicates),
                            MenuItem::action("Trim Trailing Whitespace", TrimWhitespace)
                        ],
                    })
                ],
            },
            // Selection Menu
            Menu {
                name: "Selection".into(),
                items: vec![
                    MenuItem::action("Select Line", SelectLine),
                    MenuItem::action("Select Word", SelectWord),
                    MenuItem::action("Select Scope", SelectScope),
                    MenuItem::separator(),
                    MenuItem::action("Expand Selection", ExpandSelection),
                    MenuItem::action("Shrink Selection", ShrinkSelection),
                    MenuItem::separator(),
                    MenuItem::action("Add Cursor Above", AddCursorAbove),
                    MenuItem::action("Add Cursor Below", AddCursorBelow),
                    MenuItem::action("Add Cursor at Line Ends", AddCursorLineEnds),
                    MenuItem::separator(),
                    MenuItem::action("Select All Occurrences", SelectAllOccurrences),
                    MenuItem::action("Select Next Occurrence", SelectNextOccurrence),
                    MenuItem::action("Skip Occurrence", SkipOccurrence)
                ],
            },
            // View Menu
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: "Panels".into(),
                        items: vec![
                            MenuItem::action("Explorer", ToggleExplorer),
                            MenuItem::action("Scene Hierarchy", ToggleHierarchy),
                            MenuItem::action("Inspector", ToggleInspector),
                            MenuItem::action("Asset Browser", ToggleAssetBrowser),
                            MenuItem::separator(),
                            MenuItem::action("Console", ToggleConsole),
                            MenuItem::action("Output", ToggleOutput),
                            MenuItem::action("Problems", ToggleProblems),
                            MenuItem::action("Debug", ToggleDebug),
                            MenuItem::separator(),
                            MenuItem::action("Profiler", ToggleProfiler),
                            MenuItem::action("Memory Analyzer", ToggleMemoryAnalyzer),
                            MenuItem::action("Network", ToggleNetwork)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Editor Layout".into(),
                        items: vec![
                            MenuItem::action("Split Horizontal", SplitHorizontal),
                            MenuItem::action("Split Vertical", SplitVertical),
                            MenuItem::separator(),
                            MenuItem::action("Single Column", SingleColumn),
                            MenuItem::action("Two Columns", TwoColumns),
                            MenuItem::action("Three Columns", ThreeColumns),
                            MenuItem::separator(),
                            MenuItem::action("Reset Layout", ResetLayout),
                            MenuItem::action("Save Layout", SaveLayout)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Command Palette", CommandPalette),
                    MenuItem::action("Quick Open", QuickOpen),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Zoom".into(),
                        items: vec![
                            MenuItem::action("Zoom In", ZoomIn),
                            MenuItem::action("Zoom Out", ZoomOut),
                            MenuItem::action("Reset Zoom", ResetZoom)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Minimap", ToggleMinimap),
                    MenuItem::action("Toggle Line Numbers", ToggleLineNumbers),
                    MenuItem::action("Toggle Breadcrumbs", ToggleBreadcrumbs),
                    MenuItem::action("Toggle Whitespace", ToggleWhitespace),
                    MenuItem::separator(),
                    MenuItem::action("Enter Fullscreen", ToggleFullscreen),
                    MenuItem::action("Zen Mode", ToggleZenMode)
                ],
            },
            // Go Menu
            Menu {
                name: "Go To".into(),
                items: vec![
                    MenuItem::action("Go to File", GoToFile),
                    MenuItem::action("Go to Symbol", GoToSymbol),
                    MenuItem::action("Go to Line", GoToLine),
                    MenuItem::separator(),
                    MenuItem::action("Go to Definition", GoToDefinition),
                    MenuItem::action("Go to Type Definition", GoToTypeDefinition),
                    MenuItem::action("Go to Implementation", GoToImplementation),
                    MenuItem::action("Go to References", GoToReferences),
                    MenuItem::separator(),
                    MenuItem::action("Go Back", GoBack),
                    MenuItem::action("Go Forward", GoForward),
                    MenuItem::action("Go to Last Edit", GoToLastEdit),
                    MenuItem::separator(),
                    MenuItem::action("Next Problem", NextProblem),
                    MenuItem::action("Previous Problem", PreviousProblem)
                ],
            },
            // Project Menu
            Menu {
                name: "Project".into(),
                items: vec![
                    MenuItem::action("Project Settings", ProjectSettings),
                    MenuItem::action("Build Settings", BuildSettings),
                    MenuItem::action("Package Settings", PackageSettings),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Dependencies".into(),
                        items: vec![
                            MenuItem::action("Add Dependency", AddDependency),
                            MenuItem::action("Update Dependencies", UpdateDependencies),
                            MenuItem::action("Remove Unused", RemoveUnusedDeps),
                            MenuItem::separator(),
                            MenuItem::action("Cargo.toml", OpenCargoToml)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Generate Documentation", GenerateDocs),
                    MenuItem::action("Run Cargo Check", RunCargoCheck),
                    MenuItem::action("Run Clippy", RunClippy),
                    MenuItem::action("Format Project", FormatProject)
                ],
            },
            // Build Menu
            Menu {
                name: "Build".into(),
                items: vec![
                    MenuItem::action("Build Project", Build),
                    MenuItem::action("Build & Run", BuildAndRun),
                    MenuItem::action("Build Release", BuildRelease),
                    MenuItem::separator(),
                    MenuItem::action("Rebuild All", Rebuild),
                    MenuItem::action("Clean Build", Clean),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Build Configuration".into(),
                        items: vec![
                            MenuItem::action("Debug", BuildDebug),
                            MenuItem::action("Release", BuildRelease),
                            MenuItem::action("Release with Debug Info", BuildReleaseDebug),
                            MenuItem::separator(),
                            MenuItem::action("Profile", BuildProfile)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Cancel Build", CancelBuild),
                    MenuItem::action("Show Build Output", ShowBuildOutput)
                ],
            },
            // Run Menu
            Menu {
                name: "Run".into(),
                items: vec![
                    MenuItem::action("Run Project", RunProject),
                    MenuItem::action("Run Without Debugging", RunWithoutDebug),
                    MenuItem::action("Stop", StopExecution),
                    MenuItem::action("Restart", RestartExecution),
                    MenuItem::separator(),
                    MenuItem::action("Debug Project", DebugProject),
                    MenuItem::Submenu(Menu {
                        name: "Debug".into(),
                        items: vec![
                            MenuItem::action("Start Debugging", StartDebug),
                            MenuItem::action("Stop Debugging", StopDebug),
                            MenuItem::action("Restart Debugging", RestartDebug),
                            MenuItem::separator(),
                            MenuItem::action("Step Over", StepOver),
                            MenuItem::action("Step Into", StepInto),
                            MenuItem::action("Step Out", StepOut),
                            MenuItem::action("Continue", Continue),
                            MenuItem::separator(),
                            MenuItem::action("Toggle Breakpoint", ToggleBreakpoint),
                            MenuItem::action("Disable All Breakpoints", DisableBreakpoints),
                            MenuItem::action("Remove All Breakpoints", RemoveBreakpoints)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Tests".into(),
                        items: vec![
                            MenuItem::action("Run All Tests", RunTests),
                            MenuItem::action("Run Failed Tests", RunFailedTests),
                            MenuItem::action("Run Tests in File", RunFileTests),
                            MenuItem::separator(),
                            MenuItem::action("Debug Test", DebugTest),
                            MenuItem::action("Coverage", RunCoverage)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Benchmarks".into(),
                        items: vec![
                            MenuItem::action("Run Benchmarks", RunBenchmarks),
                            MenuItem::action("Compare Benchmarks", CompareBenchmarks),
                            MenuItem::action("Profile Build", ProfileBuild)
                        ],
                    })
                ],
            },
            // Engine Menu
            Menu {
                name: "Engine".into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: "Scene".into(),
                        items: vec![
                            MenuItem::action("New Scene", NewScene),
                            MenuItem::action("Open Scene", OpenScene),
                            MenuItem::action("Save Scene", SaveScene),
                            MenuItem::action("Save Scene As", SaveSceneAs),
                            MenuItem::separator(),
                            MenuItem::action("Play Scene", PlayScene),
                            MenuItem::action("Pause Scene", PauseScene),
                            MenuItem::action("Stop Scene", StopScene),
                            MenuItem::separator(),
                            MenuItem::action("Scene Settings", SceneSettings)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: "GameObject".into(),
                        items: vec![
                            MenuItem::action("Create Empty", CreateEmpty),
                            MenuItem::separator(),
                            MenuItem::Submenu(Menu {
                                name: "3D Object".into(),
                                items: vec![
                                    MenuItem::action("Cube", CreateCube),
                                    MenuItem::action("Sphere", CreateSphere),
                                    MenuItem::action("Capsule", CreateCapsule),
                                    MenuItem::action("Cylinder", CreateCylinder),
                                    MenuItem::action("Plane", CreatePlane),
                                    MenuItem::separator(),
                                    MenuItem::action("Terrain", CreateTerrain)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "2D Object".into(),
                                items: vec![
                                    MenuItem::action("Sprite", CreateSprite),
                                    MenuItem::action("Tilemap", CreateTilemap),
                                    MenuItem::action("Particle System 2D", CreateParticles2D)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "Light".into(),
                                items: vec![
                                    MenuItem::action("Directional Light", CreateDirectionalLight),
                                    MenuItem::action("Point Light", CreatePointLight),
                                    MenuItem::action("Spot Light", CreateSpotLight),
                                    MenuItem::action("Area Light", CreateAreaLight)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "Audio".into(),
                                items: vec![
                                    MenuItem::action("Audio Source", CreateAudioSource),
                                    MenuItem::action("Audio Listener", CreateAudioListener)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "Effects".into(),
                                items: vec![
                                    MenuItem::action("Particle System", CreateParticleSystem),
                                    MenuItem::action("Visual Effect", CreateVFX),
                                    MenuItem::action("Post Processing", CreatePostProcessing)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "Camera".into(),
                                items: vec![
                                    MenuItem::action("Camera", CreateCamera),
                                    MenuItem::action("Orthographic Camera", CreateOrthoCamera)
                                ],
                            }),
                            MenuItem::Submenu(Menu {
                                name: "UI".into(),
                                items: vec![
                                    MenuItem::action("Canvas", CreateCanvas),
                                    MenuItem::action("Panel", CreatePanel),
                                    MenuItem::action("Button", CreateButton),
                                    MenuItem::action("Text", CreateText),
                                    MenuItem::action("Image", CreateImage),
                                    MenuItem::action("Slider", CreateSlider),
                                    MenuItem::action("Input Field", CreateInputField)
                                ],
                            })
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Physics".into(),
                        items: vec![
                            MenuItem::action("Add Rigidbody", AddRigidbody),
                            MenuItem::action("Add Collider", AddCollider),
                            MenuItem::action("Add Character Controller", AddCharacterController),
                            MenuItem::separator(),
                            MenuItem::action("Physics Settings", PhysicsSettings),
                            MenuItem::action("Collision Matrix", CollisionMatrix)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: "Rendering".into(),
                        items: vec![
                            MenuItem::action("Render Settings", RenderSettings),
                            MenuItem::action("Lighting Settings", LightingSettings),
                            MenuItem::action("Quality Settings", QualitySettings),
                            MenuItem::separator(),
                            MenuItem::action("Bake Lighting", BakeLighting),
                            MenuItem::action("Clear Baked Data", ClearBakedData),
                            MenuItem::separator(),
                            MenuItem::action("Frame Debugger", FrameDebugger),
                            MenuItem::action("Profiler", EngineProfiler)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Package Manager", PackageManager),
                    MenuItem::action("Asset Store", AssetStore)
                ],
            },
            // Assets Menu
            Menu {
                name: "Assets".into(),
                items: vec![
                    MenuItem::action("Create Asset", CreateAsset),
                    MenuItem::action("Import Asset", ImportAsset),
                    MenuItem::separator(),
                    MenuItem::Submenu(Menu {
                        name: "Create".into(),
                        items: vec![
                            MenuItem::action("Material", CreateMaterial),
                            MenuItem::action("Shader", CreateShader),
                            MenuItem::action("Texture", CreateTexture),
                            MenuItem::separator(),
                            MenuItem::action("Animation Clip", CreateAnimClip),
                            MenuItem::action("Animator Controller", CreateAnimController),
                            MenuItem::separator(),
                            MenuItem::action("Audio Mixer", CreateAudioMixer),
                            MenuItem::action("Render Texture", CreateRenderTexture),
                            MenuItem::separator(),
                            MenuItem::action("Prefab", CreatePrefab),
                            MenuItem::action("ScriptableObject", CreateScriptableObject)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Refresh", RefreshAssets),
                    MenuItem::action("Reimport", ReimportAssets),
                    MenuItem::action("Reimport All", ReimportAllAssets),
                    MenuItem::separator(),
                    MenuItem::action("Find References", FindAssetReferences),
                    MenuItem::action("Find Missing References", FindMissingReferences)
                ],
            },
            // Tools Menu
            Menu {
                name: "Tools".into(),
                items: vec![
                    MenuItem::Submenu(Menu {
                        name: "Rust".into(),
                        items: vec![
                            MenuItem::action("Cargo Commands", CargoCommands),
                            MenuItem::action("Generate Rust-Analyzer", GenerateRustAnalyzer),
                            MenuItem::separator(),
                            MenuItem::action("Expand Macro", ExpandMacro),
                            MenuItem::action("Show Syntax Tree", ShowSyntaxTree),
                            MenuItem::action("Show HIR", ShowHIR),
                            MenuItem::separator(),
                            MenuItem::action("Inline Variable", InlineVariable),
                            MenuItem::action("Extract Function", ExtractFunction),
                            MenuItem::action("Extract Variable", ExtractVariable)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: "Shader".into(),
                        items: vec![
                            MenuItem::action("Shader Editor", ShaderEditor),
                            MenuItem::action("Compile Shader", CompileShader),
                            MenuItem::action("Shader Variants", ShaderVariants),
                            MenuItem::separator(),
                            MenuItem::action("SPIR-V Disassembly", SPIRVDisassembly)
                        ],
                    }),
                    MenuItem::Submenu(Menu {
                        name: "Animation".into(),
                        items: vec![
                            MenuItem::action("Animation Window", AnimationWindow),
                            MenuItem::action("Animator", AnimatorWindow),
                            MenuItem::action("Timeline", Timeline)
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Material Editor", MaterialEditor),
                    MenuItem::action("Terrain Tools", TerrainTools),
                    MenuItem::action("Particle Editor", ParticleEditor),
                    MenuItem::action("Audio Mixer", AudioMixerWindow),
                    MenuItem::separator(),
                    MenuItem::action("Version Control", VersionControl),
                    MenuItem::action("Task Manager", TaskManager),
                    MenuItem::action("Extensions", Extensions)
                ],
            },
            // Window Menu
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Minimize", Minimize),
                    MenuItem::action("Zoom", Zoom),
                    MenuItem::separator(),
                    MenuItem::action("Bring All to Front", BringAllToFront),
                    MenuItem::separator(),
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::action("Close Window", CloseWindow)
                ],
            },
            // Help Menu
            Menu {
                name: "Help".into(),
                items: vec![
                    MenuItem::action("Documentation", ShowDocumentation),
                    MenuItem::action("API Reference", ShowAPIReference),
                    MenuItem::action("Tutorials", ShowTutorials),
                    MenuItem::separator(),
                    MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
                    MenuItem::action("Command Palette", CommandPalette),
                    MenuItem::separator(),
                    MenuItem::action("Report Issue", ReportIssue),
                    MenuItem::action("View Logs", ViewLogs),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates", CheckUpdates),
                    MenuItem::action("Release Notes", ReleaseNotes),
                    MenuItem::separator(),
                    MenuItem::action("About Pulsar Engine", AboutApp)
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
    child: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
    _subscriptions: Vec<Subscription>,
}

impl AppTitleBar {
    pub fn new(
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Self {
        init_app_menus(title, cx);

        let app_menu_bar = AppMenuBar::new(window, cx);
        let locale_selector = cx.new(|cx| LocaleSelector::new(window, cx));
        let font_size_selector = cx.new(|cx| FontSizeSelector::new(window, cx));
        let theme_switcher = cx.new(|cx| ThemeSwitcher::new(cx));

        Self {
            app_menu_bar,
            locale_selector,
            font_size_selector,
            theme_switcher,
            child: Rc::new(|_, _| div().into_any_element()),
            _subscriptions: vec![],
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
        _: &mut Context<Self>
    ) {
        set_locale(&locale.0);
        window.refresh();
    }
}

impl Render for LocaleSelector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();
        let locale = locale().to_string();

        div()
            .id("locale-selector")
            .track_focus(&focus_handle)
            .on_action(cx.listener(Self::on_select_locale))
            .child(
                Button::new("btn")
                    .small()
                    .ghost()
                    .icon(IconName::Globe)
                    .popup_menu(move |this, _, _| {
                        this.menu_with_check(
                            "English",
                            locale == "en",
                            Box::new(SelectLocale("en".into()))
                        ).menu_with_check(
                            "简体中文",
                            locale == "zh-CN",
                            Box::new(SelectLocale("zh-CN".into()))
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
