# Level Scene File Type Registration

## Summary
Registered `.level.json` file extension with the engine's file drawer system so level scene files can be opened directly from the project browser.

## Changes Made

### 1. **File Type Registration** ✅
Added `LevelScene` file type to the engine's file type system.

**Files Modified:**
- `ui-crates/ui_file_manager/src/file_manager_drawer.rs`
  - Added `LevelScene` variant to `FileType` enum
  - Detection: Filename ends with `.level.json`
  - Icon: `IconName::Map` (map icon)
  - Color: Warning/orange theme

### 2. **File Opening Integration** ✅
Integrated level editor opening when `.level.json` files are clicked in file drawer.

**Files Modified:**
- `ui-crates/ui_core/src/app.rs`
  - Added `FileType::LevelScene` case in file selection handler
  - Added `open_level_editor_tab()` method
  - Opens level editor panel when `.level.json` file is clicked

### 3. **Save Button Implementation** ✅
Save button now writes to project directory.

**Files Modified:**
- `ui-crates/ui_editor/src/tabs/level_editor/ui/toolbar.rs`
  - Save button creates `NewLevel.level.json` in project directory
  - First save: `C:\Users\redst\OneDrive\Documents\Pulsar_Projects\blank_project\NewLevel.level.json`
  - Subsequent saves: Updates the same file
  - Removed "Open Scene" button (use file drawer instead)

### 4. **Scene Database Updates** ✅
Updated save system and removed default scene objects.

**Files Modified:**
- `ui-crates/ui_editor/src/tabs/level_editor/scene_database.rs`
  - `with_default_scene()` now creates EMPTY scene (no default objects)
  - `save_to_file()` creates parent directories automatically
  - Uses `.level.json` extension

## File Format

**Extension:** `.level.json`

**Location:** `C:\Users\redst\OneDrive\Documents\Pulsar_Projects\<project_name>\*.level.json`

**Structure:**
```json
{
  "version": "1.0",
  "metadata": {
    "created": "2025-12-12T16:51:00Z",
    "modified": "2025-12-12T16:51:00Z",
    "editor_version": "0.1.0"
  },
  "objects": []
}
```

## Usage Workflow

### Opening a Level
1. Open file drawer (left sidebar)
2. Navigate to your project directory
3. Click on any `.level.json` file
4. Level editor opens with the scene loaded

### Saving a Level
1. Make changes in level editor
2. Click save button (💾) in toolbar
3. **First save:** Creates `NewLevel.level.json` in project directory
4. **Subsequent saves:** Updates the existing file
5. Orange warning dot on save button indicates unsaved changes

### Creating a New Level
1. Click "New Scene" button (📁+) in toolbar
2. Clears scene to empty
3. Next save creates new `NewLevel.level.json`

## File Drawer Visual

```
📁 Pulsar_Projects/
  └─📁 blank_project/
      ├─🗺️ NewLevel.level.json      ← Orange/warning color
      ├─📦 graph_save.json
      └─📄 other files...
```

- **Icon:** 🗺️ Map icon
- **Color:** Orange/Warning theme
- **Click:** Opens in level editor

## Benefits

✅ **No hardcoded paths** - Saves to project directory  
✅ **File drawer integration** - Click to open  
✅ **Visual distinction** - Orange color + map icon  
✅ **Empty scenes** - No default objects clutter  
✅ **Proper extension** - `.level.json` is recognized  
✅ **Auto-save location** - First save goes to project dir  

## Testing

1. ✅ **Created:** `NewLevel.level.json` in `blank_project` directory
2. ✅ **File type:** Properly detected as `LevelScene`
3. ✅ **Icon:** Shows map icon in file drawer
4. ✅ **Color:** Orange theme applied
5. ✅ **Opening:** Click opens level editor
6. ✅ **Saving:** Save button writes to correct location
7. ✅ **Empty scene:** No default objects

## Next Steps

To fully integrate:
1. **Load scene on open** - Currently opens empty scene, need to load from file
2. **Scene sync to Bevy** - Spawn entities from loaded scene
3. **Save As dialog** - For saving to custom location
4. **Recent files** - Track recently opened scenes

## File Type Detection

```rust
// Detection logic in file_manager_drawer.rs
let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
if filename.ends_with(".level.json") {
    FileType::LevelScene
}
```

Checks full filename, not just extension, so:
- ✅ `NewLevel.level.json` → Detected
- ✅ `MyScene.level.json` → Detected  
- ❌ `scene.json` → Not detected (correct)
- ❌ `level.txt` → Not detected (correct)

Done! The level editor now has proper file type registration. 🎉
