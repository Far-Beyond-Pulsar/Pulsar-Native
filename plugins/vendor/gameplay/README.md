# Plugin_Gameplay

`pulsar.gameplay` ("Gameplay") is a plugin of Pulsar-Native. Its one job is to
register the gameplay roles as scene defaults.

- **The four roles** (Game Mode, Game State, Player Controller, Player
  Character) are registered here, not in the engine. The traits and ids stay
  the engine's (`pulsar_traits::builtin`); disabling the plugin removes the
  roles and their World Settings pickers.
- **A native default player character.** When a level assigns no Player
  Character class, the session spawns a scene object with a
  `CharacterComponent` (which places its controller and input) and a camera,
  through a `RoleDefaultRegistration`. A level that assigns the role keeps its
  own class.
- **Input.** The plugin requires the input and character `class` contracts
  and links neither crate. The input plugin's possession hook puts the level's
  `player_input_mapping` to work on the pawn; this plugin hears the pawn's
  `input::Move.*` and `input::Jump.started` events and writes the character's
  `move_input` and `jump`.

A playable game today: a level with a player input mapping that has `Move` (2D,
x right, y forward) and `Jump` actions; the example `gameplay.inputmap` is one.

Not here yet: a player start (the pawn spawns at the origin), a camera rig
beyond a `CameraComponent` on the pawn, mouse look, and a default mapping when
the level sets none.
