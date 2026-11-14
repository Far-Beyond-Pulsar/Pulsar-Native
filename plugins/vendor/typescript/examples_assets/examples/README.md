# TypeScript class examples

TypeScript is a scripting language, not an editor: a class is a directory
`src/classes/<Class>/` holding `class.ts`, which the plugin compiles to an
engine script module. Each example here is one such class directory. Copy one
into your project's `src/classes/` to use it; the compiler writes
`events/.build/` and `class.schema.json` next to it (commit the schema, it
keeps each field's identity across edits).

| Class | Shows |
|---|---|
| `Door/class.ts` | Fields, `async` methods and `await wait(seconds)` (suspends in game time) |
| `Counter/class.ts` | Control flow: `for`, `while`, `if`, `break`, `continue`, and `int` against `number` |
| `Patrol/class.ts` | Engine types and natives: `Vec3`, `tick(delta)` and `string` fields |
| `Hero/class.ts` | Migration: `@renamedFrom` and `migrate(from)` after a class changes |

The language is a subset of TypeScript checked against the engine's native
registry, with no JavaScript runtime: no imports, generics, closures, arrays or
objects. `Pulsar/types/pulsar.d.ts`, generated into the project, lists exactly
what is available.
