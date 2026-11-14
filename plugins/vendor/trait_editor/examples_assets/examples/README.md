# Trait examples

Read-only examples of `*.trait.json` files, served by the Trait Editor plugin.
Open one to see the Trait Editor on it, or save a copy into your project's
`types/traits/` folder and change it.

| File | Shows |
|---|---|
| `Damageable.trait.json` | The smallest useful trait: a few methods with primitive parameters and return values |
| `Interactable.trait.json` | `Path` type references and `defaultBody` implementations |
| `Lootable.trait.json` | `supertraits` (extending another trait by id) and `meta` |

A trait file is JSON with `schemaVersion`, `typeKind: "trait"`, a stable `id`
(Blueprints reference a trait by it, so keep it when renaming), `name`,
`displayName`, optional `description` and `supertraits`, the `methods`, and a
free-form `meta` object. Each method has a `signature` (`params` and `return`,
each type a `Primitive`, `Path` or `AliasRef`), an optional `doc` and an
optional `defaultBody`. JSON has no comments, so documentation goes in the
`description` and `doc` fields.
