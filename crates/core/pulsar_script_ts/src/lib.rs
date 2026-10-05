//! TypeScript for the engine's script VM.
//!
//! A script class is one TypeScript class, in `src/classes/<Class>/class.ts`,
//! compiled to the same [`Module`](pulsar_script_vm::Module) a Blueprint
//! compiles to. There is no JavaScript runtime: the code is type-checked
//! against the native registry and lowered to bytecode, so it runs on the same
//! VM, with the same instances, limits, waiting and hot reload.
//!
//! ```ts
//! export default class Door extends ScriptClass {
//!     speed: number = 2.5;
//!     opened: int = 0;
//!     position: Vec3 = Vec3.new_(0, 0, 0);
//!
//!     async open(): Promise<void> {
//!         await wait(1.5);                       // suspends in game time
//!         this.opened += 1;
//!         this.position = this.position.add(Vec3.new_(0, this.speed, 0));
//!     }
//!
//!     tick(delta: number): void { /* ... */ }
//! }
//! ```
//!
//! # Why oxc
//!
//! The parser is [`oxc`](https://oxc.rs) (`oxc_parser`, MIT, pinned to one
//! release). It parses TypeScript natively and keeps spans for every node,
//! which the debug info needs, and it is small enough to build with the editor;
//! `swc` brings a much larger dependency tree for features (a JS transform
//! pipeline, a type checker we cannot use) this does not need. A parser is not a
//! type checker: checking is done here, against the script type system, and
//! that is the only checking that matters, because it is what the verifier
//! enforces.
//!
//! # The subset
//!
//! Supported: one class with typed fields (initialised with literals) and
//! methods; `let`/`const`, assignment and compound assignment, `++`/`--`, `if`,
//! `while`, `for`, `break`, `continue`, `return`, calls, `&&`, `||`, `!`,
//! `?:`, arithmetic and comparison; `await wait(seconds)` and awaiting another
//! `async` method of the class; `x as int|number|string` conversions; native
//! functions as `namespace.name(..)` and methods of engine types (`v.add(w)`,
//! `v.x`).
//!
//! Not supported, and rejected with a message rather than ignored: imports,
//! generics, closures and arrow functions, object and array literals,
//! destructuring, template literals, `switch`, `try`/`throw`, `for..of`/`in`,
//! labels, classes other than the script class, getters/setters, static
//! members, constructors, optional chaining, `any`, unions, `Promise` outside
//! an `async` return, and arbitrary promises or timers. Anything that would
//! need a JavaScript runtime stays out.
//!
//! # Numbers
//!
//! The VM has `int` (64-bit) and `float`. TypeScript's `number` is the VM
//! `float`; `int` is a declared alias for the engine integer. A whole-number
//! literal is an `int` unless a `number` is expected (`let x: number = 5`,
//! `this.speed * 2`). The two never mix silently: convert with `x as number`
//! or `x as int`. Integer arithmetic follows the VM's policy (wrapping, or an
//! error where the project asks for checked arithmetic).
//!
//! # Identity and migration
//!
//! Fields keep a stable id in `class.schema.json` ([`ClassSchema`]); see that
//! module. `migrate(fromVersion: int): void` runs after the class version
//! rises, reading old values through the `migration` natives.
//!
//! # Declarations
//!
//! [`declarations`] generates the `.d.ts` for the registry the compiler checks
//! against, so editors and the compiler see the same natives.

mod declarations;
mod diagnostic;
mod lower;
mod schema;

pub use declarations::{declarations, ts_name, ts_type};
pub use diagnostic::{Diagnostic, Severity};
pub use lower::{compile_class, ClassSource, Compiled};
pub use schema::{ClassSchema, DeclaredField, SchemaField};
