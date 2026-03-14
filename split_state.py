import re

with open(r'D:\Documents\GitHub\genesis\Pulsar-Native\crates\ui\src\input\state.rs', 'r', encoding='utf-8') as f:
    content = f.read()

lines = content.split('\n')

def section(start, end):
    """Extract lines start..end (1-based, inclusive), joined with newlines."""
    return '\n'.join(lines[start-1:end])

# The original use statements (copy to all files)
USE_STATEMENTS = section(1, 39)

# ===== mod.rs =====
mod_content = '''mod core;
mod events;
mod layout;
mod lsp;
mod ime;

pub use core::{
    LineHighlight, Enter, InputEvent, InputState, CONTEXT, init, LastLayout,
    Backspace, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine,
    DeleteToPreviousWordStart, DeleteToNextWordEnd, Indent, Outdent,
    IndentInline, OutdentInline, MoveUp, MoveDown, MoveLeft, MoveRight,
    MoveHome, MoveEnd, MovePageUp, MovePageDown,
    SelectUp, SelectDown, SelectLeft, SelectRight, SelectAll,
    SelectToStartOfLine, SelectToEndOfLine, SelectToStart, SelectToEnd,
    SelectToPreviousWordStart, SelectToNextWordEnd,
    ShowCharacterPalette, Copy, Cut, Paste, Undo, Redo,
    MoveToStartOfLine, MoveToEndOfLine, MoveToStart, MoveToEnd,
    MoveToPreviousWord, MoveToNextWord, Escape, ToggleCodeActions, Search, GoToDefinition,
};
'''

# ===== core.rs =====
# Type defs + struct (lines 41-358)
# First impl block sections (various scattered methods)
# Focusable impl + Render impl

core_parts = [
    USE_STATEMENTS,
    '',
    section(41, 358),    # types, struct, EventEmitter impl
    '',
    'impl InputState {',
    section(361, 640),   # rope_starts_with through set_line_highlights (line 640 is blank before line_and_position_for_offset)
    section(674, 918),   # set_value through focus (skip lsp method 649-672)
    section(1692, 1702), # cursor()
    section(1909, 1955), # offset_from_utf16 through show_cursor
    section(2023, 2073), # is_valid_input through set_mask_pattern
    section(2075, 2100), # set_input_bounds, selected_text
    '}',
    '',
    section(2406, 2410), # impl Focusable
    '',
    section(2412, 2428), # impl Render
]
core_content = '\n'.join(core_parts)

# Apply visibility changes in core.rs
# pub(super) -> pub(in crate::input) for items visible to input module
core_content = core_content.replace('pub(super)', 'pub(in crate::input)')
# Private methods that need cross-submodule access within state/
core_content = core_content.replace('\n    fn replace_text(', '\n    pub(in crate::input::state) fn replace_text(')
core_content = core_content.replace('\n    fn is_valid_input(', '\n    pub(in crate::input::state) fn is_valid_input(')
core_content = core_content.replace('\n    fn rope_starts_with(', '\n    pub(in crate::input::state) fn rope_starts_with(')

# ===== events.rs =====
events_parts = [
    USE_STATEMENTS,
    'use super::core::InputState;',
    '',
    'impl InputState {',
    section(920, 1422),  # select_left through escape
    section(1613, 1651), # show_character_palette through paste
    section(1654, 1687), # push_history through redo
    section(1957, 2021), # on_focus through on_drag_move
    '}',
]
events_content = '\n'.join(events_parts)
events_content = events_content.replace('pub(super)', 'pub(in crate::input)')
events_content = events_content.replace('\n    fn push_history(', '\n    pub(in crate::input::state) fn push_history(')

# ===== layout.rs =====
layout_parts = [
    USE_STATEMENTS,
    'use super::core::InputState;',
    '',
    'impl InputState {',
    section(1424, 1535), # on_mouse_down through on_scroll_wheel
    section(1538, 1611), # update_scroll_offset, scroll_to
    section(1704, 1906), # index_for_mouse_position through unselect
    '}',
]
layout_content = '\n'.join(layout_parts)
layout_content = layout_content.replace('pub(super)', 'pub(in crate::input)')

# ===== lsp.rs =====
lsp_parts = [
    USE_STATEMENTS,
    'use super::core::InputState;',
    '',
    'impl InputState {',
    section(649, 672),   # line_and_position_for_offset
    section(2102, 2125), # range_to_bounds
    section(2131, 2145), # replace_text_in_lsp_range
    '}',
]
lsp_content = '\n'.join(lsp_parts)
lsp_content = lsp_content.replace('pub(super)', 'pub(in crate::input)')

# ===== ime.rs =====
ime_parts = [
    USE_STATEMENTS,
    'use super::core::InputState;',
    '',
    'impl InputState {',
    section(2151, 2161), # replace_text_in_range_silent (2162 is `}` closing main impl)
    '}',
    '',
    section(2164, 2404), # impl EntityInputHandler
]
ime_content = '\n'.join(ime_parts)
ime_content = ime_content.replace('pub(super)', 'pub(in crate::input)')

# Write files
base = r'D:\Documents\GitHub\genesis\Pulsar-Native\crates\ui\src\input\state'

with open(f'{base}\\mod.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(mod_content)

with open(f'{base}\\core.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(core_content)

with open(f'{base}\\events.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(events_content)

with open(f'{base}\\layout.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(layout_content)

with open(f'{base}\\lsp.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(lsp_content)

with open(f'{base}\\ime.rs', 'w', encoding='utf-8', newline='\n') as f:
    f.write(ime_content)

# Verify the files were created and check sizes
import os
for fname in ['mod.rs', 'core.rs', 'events.rs', 'layout.rs', 'lsp.rs', 'ime.rs']:
    path = f'{base}\\{fname}'
    size = os.path.getsize(path)
    lcount = open(path, encoding='utf-8').read().count('\n')
    print(f"{fname}: {size} bytes, ~{lcount} lines")

print("Done!")
