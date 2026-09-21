---
name: module-setup
description: Create or reorganize Rust modules following the crate Module Declaration Constitution. Use when adding a new .rs file, moving files, or fixing bare mod violations.
---

## Module Declaration Constitution

Every crate MUST declare its modules exclusively through `crate_macro` macros. Manual `mod` / `pub mod` / `pub use` for module plumbing is forbidden.

| Macro | Generated code | When to use |
|---|---|---|
| `mod_pub!(X)` | `pub mod X;` | Public module, path-accessed (`crate::X::item`) |
| `mod_prv!(X)` | `mod X;` | Private crate-internal module (no re-export) |

Both macros accept multiple names: `mod_pub!(a, b, c);` expands to `pub mod a; pub mod b; pub mod c;`.

`mod_pub!` also supports visibility prefixes: `mod_pub!(crate, A)` → `pub(crate) mod A;`, `mod_pub!(super, A)` → `pub(super) mod A;`, `mod_pub!(pub(self), A)` → `pub(self) mod A;`.

## Rules

1. **Single-call-per-type**: Each macro (`mod_pub!`, `mod_prv!`) appears at most once per file. Merge same-type calls into one list.
2. **Different types can coexist**: One `mod_pub!` + one `mod_prv!` is fine.
3. **Inline modules exempt**: `mod func3 { ... }`, `mod tests { ... }` don't need macros.
4. **`as` alias exception**: `pub use LongName as Short;` is OK after `mod_pub!`.
5. **Selective re-exports**: Control visibility inside the module with `pub`/`pub(crate)`. A module declared with `mod_pub!` exposes all its `pub` items under the module path; a `mod_prv!` module is invisible outside the crate.

## Workflow

### Adding a new public module

1. Create `src/new_module.rs`
2. Find the existing `mod_pub!` call in `lib.rs` (or `mod.rs` for sub-modules)
3. Add the new name to the list
4. If no `mod_pub!` exists yet, create one: `crate_macro::mod_pub!(new_module);`
5. Access items as `crate::new_module::item`

### Adding a new private module

1. Create `src/internal.rs`
2. Find the existing `mod_prv!` call in `lib.rs`
3. Add the new name to the list
4. If no `mod_prv!` exists, create one: `crate_macro::mod_prv!(internal);`

### Common mistakes to catch

- Bare `mod X;` without macro → violation
- Separate `pub use X::*;` when you meant to flatten a module → `crate_macro` has no flattening macro; if you need flat re-export, add an explicit `pub use crate::X::*;` after the `mod_pub!` call (with a comment noting the exception)
- Two `mod_pub!` calls instead of one merged call → violation of single-call-per-type
- Using `mod_prv!` for a module you need to expose publicly → use `mod_pub!` instead

## Exception

`crate_macro/src/lib.rs` uses bare `mod module;` — this is the ONLY allowed exception (bootstrap problem: the macros are defined inside that module).
