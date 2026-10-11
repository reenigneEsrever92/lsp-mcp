# Go test project (`gopls`)

A small Go module split across two packages, used to exercise `lsp-mcp` against
`gopls`.

```text
go.mod              module example.com/greeter (go 1.22)
greeter/greeter.go  package greeter: type Person + New/Greet
main.go             package main: imports greeter, plus one deliberate error
```

## Run it

```sh
cd test-projects/go
lsp-mcp gopls
```

`gopls` discovers the module from `go.mod` in the working directory. Nothing
needs to be downloaded — the module has no external dependencies.

## What to try

- `definition` on `greeter.New` in `main.go` → the function in
  `greeter/greeter.go` (cross-package).
- `references` on `Person` → its definition, the `New` constructor, and the
  `Greet` method receiver.
- `hover` on `Person` → the `// Person is someone we can greet.` doc comment.
- `documentSymbol` on `greeter/greeter.go` → `Person`, its fields, and the
  functions/methods.
- `workspaceSymbol` for `Greet` → the free function and the method.
- `diagnostics` on `main.go` → `declared and not used: unused`.
- `codeAction` on the unused variable → gopls' quick fixes.
- `rename` on `Person` → updates the struct and every use.
- `format` on `main.go` → gofmt edits.
