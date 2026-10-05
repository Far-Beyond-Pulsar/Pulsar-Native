# TypeScript scripting

A script class can be written in TypeScript. It compiles to the same bytecode a
Blueprint compiles to, so it runs on the same VM, with the same instances,
limits, waiting and hot reload. There is no JavaScript runtime and no `node`
at play time.

## A class

One class per directory, in `src/classes/<Class>/class.ts`. The class has the
directory's name.

```ts
export default class Door extends ScriptClass {
    speed: number = 2.5;                       // a field: per-instance state
    opened: int = 0;
    position: Vec3 = Vec3.new_(0, 0, 0);

    async open(): Promise<void> {
        await wait(1.5);                       // suspends in game time
        this.opened += 1;
        this.position = this.position.add(Vec3.new_(0, this.speed, 0));
    }

    tick(delta: number): void { /* every frame */ }
}
```

- Lifecycle methods are `begin_play(): void`, `tick(delta: number): void` and
  `end_play(): void`. Every other public method is callable by name (the
  engine, other scripts, events). A method that starts with `_` or is
  `private` is not exported.
- `this.entity` is the entity the instance is bound to; `this.time` is game
  time in seconds.
- `await wait(seconds)` suspends the method (it must be `async` and return
  `Promise<void>`). `await this.other()` awaits another async method of the
  class. Nothing blocks the game thread.

## Numbers

TypeScript has one `number`; the VM has `int` (64-bit) and `float`.

| TypeScript | VM |
|---|---|
| `number` | `float` |
| `int` | `int` |
| `boolean`, `string` | `bool`, `string` |
| `Entity`, `Vec3`, `Quat`, components, ... | the engine type of that name |

A whole-number literal is an `int` unless a `number` is expected
(`let x: number = 5`, `this.speed * 2`). They never mix silently: convert with
`x as number`, `x as int` or `x as string`. Integer arithmetic wraps, or is an
error where the project asks for checked arithmetic.

## What the engine provides

Natives are functions in namespaces (`math.sqrt(x)`, `Vec3.new_(1, 2, 3)`) and
methods of engine types (`v.add(w)`, `v.length()`, `v.x`). Names that are
reserved words in TypeScript get a trailing underscore (`new_`, `string_`).

`Pulsar/types/pulsar.d.ts` lists all of them with their signatures. It is
generated from the same registry the compiler checks against, rewritten
whenever the project's TypeScript compiles, and meant to be on your editor's
include path. Autocomplete and the compiler cannot disagree.

## The supported subset

Supported: typed fields initialised with literals (or `Type.new_(numbers)`),
methods, `let`/`const`, assignment and compound assignment, `++`/`--`, `if`,
`while`, `for`, `break`, `continue`, `return`, calls, `&&`, `||`, `!`, `?:`,
arithmetic and comparison.

Rejected with an error that names the construct: imports, generics, closures
and arrow functions, object/array literals, destructuring, template literals,
`switch`, `try`/`throw`, `for..of`/`in`, labels, getters/setters, static
members, constructors (initialise fields where they are declared), optional
chaining, unions, `any`, and arbitrary promises or timers.

## Field identity, versions and migration

State carries over a hot reload, a saved game and level overrides by field
**identity**, not name. The compiler keeps each field's id in
`class.schema.json` next to the source: **commit it**.

- Rename a field without losing its value:
  `@renamedFrom("oldName") newName: int = 0;`. Without the decorator a rename
  is a removed field and a new one.
- The class version rises whenever a field is added, removed, renamed or
  retyped. After it rises, an optional `migrate(fromVersion: int): void` runs
  once per instance and may read the old values:

  ```ts
  migrate(fromVersion: int): void {
      this.shield = (migration.old_int("hp") as number) / 100;
  }
  ```

  If `migrate` fails, the reload is refused and the old class keeps running.

## Errors

Diagnostics carry `line:column` in `class.ts`, and runtime errors report the
same positions in their call stack.

## One language per class

A class has either `class.ts` or `graph_save.json`, never both. The compiled
module records its language in `events/.build/language`; a class is never
overwritten by a different language's compiler.

## Building without it

TypeScript is a plugin like Blueprints: the editor builds without it
(`--no-default-features`, or without the `typescript` feature), and
`pulsar build-scripts` includes it through the packager's `typescript` feature.
