# Plugin_Gameplay

`pulsar.gameplay` ("Gameplay") is a plugin of Pulsar-Native. Its one job is to
register the gameplay roles as scene defaults.

- **The four roles** (Game Mode, Game State, Player Controller, Player
  Character) are registered here, not in the engine. The traits and ids stay
  the engine's (`pulsar_traits::builtin`); disabling the plugin removes the
  roles and their World Settings pickers.
- **A default player character, as a class.** The plugin's folder,
  `plugins/Gameplay/` (read-only, in the file drawer), holds
  `classes/PlayerCharacter.class`: a Blueprint that implements the Player
  Character trait. A level selects it in World Settings > Player Character and
  the roles system spawns it like any other class. A level that selects no
  character spawns none: the free camera is the only fallback.
- **An input setup like Unreal's.** `input/` holds the actions `IA_Move`,
  `IA_Look`, `IA_Jump` and `IA_Sprint` and the mapping `IMC_Default` (WASD or the
  left stick, mouse or right stick, Space or the bottom face button, Shift or
  the left stick click). The character's Player Input component uses that
  mapping, and the character's own Blueprint handles the actions in five
  commented sections: the trait, Move, Jump, Sprint and Look. It drives its
  `CharacterComponent` (`set_move`, `request_jump`, `walk_speed`) and turns its
  transform for Look.

The Blueprint is written by `plugins/installed/tests/regenerate_player_character_graph.rs`
(run with `REGENERATE_PLAYER_CHARACTER=1`), which keeps the layout on a grid and
every wire consistent; a test fails when the checked-in file is stale.

The plugin requires the input and character `class` contracts and links neither
crate.

Not here yet: a player start (the pawn spawns at the origin), looking up and
down (it needs a camera rig), crouch, and a way to copy the class into a project
with one click.
