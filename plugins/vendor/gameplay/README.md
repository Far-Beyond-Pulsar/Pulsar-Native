# Plugin_Gameplay

`pulsar.gameplay` ("Gameplay") is a plugin of Pulsar-Native. Its one job is to
register the gameplay roles as scene defaults.

- **The four roles** (Game Mode, Game State, Player Controller, Player
  Character) are registered here, not in the engine. The traits and ids stay
  the engine's (`pulsar_traits::builtin`); disabling the plugin removes the
  roles and their World Settings pickers.
- **A default player character, as a class.** The plugin's folder
  `plugins/Gameplay/classes/PlayerCharacter.class` is mounted read-only: a
  Blueprint class that implements the Player Character trait, with a prefab of
  a `CharacterComponent` (and its controller and input) and a camera. A level
  selects it in World Settings > Player Character, and the roles system spawns
  it like any other class. It compiles into the project's module store; the
  folder stays read-only (copy it into `src/classes/` to change it).
- **Nothing is spawned by default.** A level that selects no character spawns
  none; the free camera is the only fallback.
- **Input.** The plugin requires the input and character `class` contracts and
  links neither crate. The input plugin's possession hook puts the level's
  `player_input_mapping` to work on the pawn; this plugin follows the possessed
  pawn (`PossessionChanged`), hears its `input::Move.*` and `input::Jump.started`
  events, and writes the character's `move_input` and `jump`.

A playable game today: select this character in World Settings and give the
level a player input mapping that has `Move` (2D, x right, y forward) and `Jump`
actions; the input plugin's example `gameplay.inputmap` is one.

Not here yet: a player start (the pawn spawns at the origin), mouse look, and a
default mapping when the level sets none.
