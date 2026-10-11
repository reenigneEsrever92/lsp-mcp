# TypeScript / JavaScript test project (`typescript-language-server`)

A small TypeScript project with one plain-JavaScript module alongside it, used
to exercise `lsp-mcp` against `typescript-language-server`.

```text
package.json   scripts and the local TypeScript toolchain
tsconfig.json  strict mode, allowJs + checkJs, noUnusedLocals
src/greeter.ts interface Person + person() + greet()
src/legacy.js  a .js module with a JSDoc type, imported by index.ts
src/index.ts   imports both, plus one deliberate warning
```

## Set up (one time)

`typescript-language-server` needs a TypeScript installation it can resolve, so
install the local dev dependencies once:

```sh
cd test-projects/typescript
npm install
```

This pulls in `typescript` and `typescript-language-server` (both listed as
`devDependencies`), so you do not need either on your global `PATH`.

## Run it

```sh
cd test-projects/typescript
lsp-mcp typescript-language-server -- --stdio
```

## What to try

- `definition` on `person`/`greet` in `src/index.ts` → `src/greeter.ts`
  (cross-file).
- `definition` on `legacyName` in `src/index.ts` → `src/legacy.js`
  (cross-language), even though it is JavaScript.
- `references` on `Person` → the interface, `person()`, and `greet()`.
- `hover` on `person` → the JSDoc `/** Builds a {@link Person} ... */`
  comment and signature.
- `documentSymbol` on `src/greeter.ts` → `Person`, `person`, `greet`.
- `workspaceSymbol` for `greet` → the exported function.
- `diagnostics` on `src/index.ts` → `'unused' is declared but its value is
  never read.` (`noUnusedLocals`).
- `codeAction` on the unused binding → TypeScript's quick fixes.
- `rename` on `Person` → updates the interface and every use.
- `format` on `src/index.ts` → server-side formatting edits.
