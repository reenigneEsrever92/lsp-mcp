# Java test project (`java-lsp`)

A tiny Maven project used to exercise `lsp-mcp` against `java-lsp`, a JVM-free
Java language server written in Rust.

```text
pom.xml                                         the greeting project
src/main/java/com/example/greeting/Person.java  class Person + greet(), with Javadoc
src/main/java/com/example/greeting/Main.java    uses Person, plus one deliberate parse error
```

## Run it

```sh
cd test-projects/java
lsp-mcp java-lsp
```

No build step is required — `java-lsp` indexes the workspace sources directly and
never invokes Maven. A JDK is optional: when `java-lsp` finds one it also indexes
the standard library, otherwise it runs on the workspace sources alone.

`java-lsp` only indexes a workspace when the client advertises a root, so
`lsp-mcp` sends its process working directory as the workspace root (see the
[root README](../../README.md)); start it from inside this directory.

## What to try

`java-lsp` currently advertises navigation and completion only, so of the
`lsp-mcp` tools these answer today:

- `definition` on `Person` in `src/main/java/com/example/greeting/Main.java` →
  the class in `.../Person.java` (cross-file).
- `references` on `Person` in `.../Person.java` → its uses in `.../Main.java`.

`hover`, `documentSymbol`, `workspaceSymbol`, `diagnostics`, and the act/modify
tools (`codeAction`, `rename`, `format`) return a clear "does not advertise …"
error until `java-lsp` grows those capabilities. `Main.java` carries a deliberate
parse error (a missing `;`) so the fixture is ready for the `diagnostics` tool
when `java-lsp` publishes diagnostics.
