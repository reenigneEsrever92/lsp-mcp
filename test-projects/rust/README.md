# Rust test project (`rust-analyzer`)

A tiny Cargo library plus binary, used to exercise `lsp-mcp` against
`rust-analyzer`.

```text
Cargo.toml    the greeter crate
src/lib.rs    struct Person + greet(), with doc comments
src/main.rs   links against lib.rs, plus one deliberate warning
```

## Run it

```sh
cd test-projects/rust
lsp-mcp rust-analyzer
```

`rust-analyzer` discovers the crate from `Cargo.toml` in the working directory.
No build step is required first — `rust-analyzer` runs `cargo check` itself and
reports the diagnostics from it.

## What to try

- `definition` on `Person::new` in `src/main.rs` → the `impl` in `src/lib.rs`
  (cross-file).
- `references` on `Person` → its definition and uses across both files.
- `hover` on `Person::new` → the `/// Builds a [Person] ...` doc comment and
  signature.
- `documentSymbol` on `src/lib.rs` → the `Person` struct, its fields, and the
  `greet` functions.
- `workspaceSymbol` for `greet` → the free function and the method.
- `diagnostics` on `src/main.rs` → `unused variable: unused`.
- `codeAction` on `unused` → "Prefix with underscore".
- `rename` on `Person` → updates the struct and every use.
- `format` on `src/lib.rs` → rustfmt edits.
