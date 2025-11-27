# Type System Demo Project

This is an example Pulsar project demonstrating the type system editors.

## Types Defined

### Structs

**Player** (`types/structs/player/`)
- Represents a player entity in the game
- Fields: id, name, health, position
- Uses both primitive types and external path types (glam::Vec3)

### Enums

**GameState** (`types/enums/game_state/`)
- Represents the current game state
- Variants: MainMenu, Playing, Paused, GameOver(u32)
- Example of enum with and without payloads

### Aliases

**PlayerRef** (`types/aliases/player_ref/`)
- Type alias for `Arc<Player>`
- Demonstrates how to create wrapper types via aliases
- Allows thread-safe shared ownership of Player instances

## How to Use

1. Open this directory as a Pulsar project
2. Navigate to the Types mount point (📐) in the file drawer
3. Explore the type definitions
4. Edit them using the visual editors
5. Save to see generated Rust code

## Generated Code

The system automatically generates Rust code in each type's `mod.rs` file:

- `types/structs/player/mod.rs` - Player struct implementation
- `types/enums/game_state/mod.rs` - GameState enum implementation
- `types/aliases/player_ref/mod.rs` - PlayerRef type alias

## Type Index

All types are tracked in `type-index/index.json` for quick lookup and dependency resolution.
