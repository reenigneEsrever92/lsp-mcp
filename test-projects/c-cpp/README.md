# C/C++ test project (`clangd`)

A small C++17 program split across a header, an implementation, and a `main`,
used to exercise `lsp-mcp` against `clangd` (which also indexes plain C).

```text
include/greeter.hpp   struct Person + two greet() overloads (declarations)
src/greeter.cpp       the greet() definitions
src/main.cpp          calls into greeter, plus one deliberate warning
.clangd               clangd configuration (standard, warnings, include path)
CMakeLists.txt        builds the project with CMake
```

## Run it

```sh
cd test-projects/c-cpp
lsp-mcp clangd
```

`clangd` needs to know your compiler flags to resolve `#include "greeter.hpp"`
and to report warnings. Rather than a `compile_commands.json` (whose paths are
machine-specific and do not survive a clone), this project checks in a
[`.clangd`](.clangd) file:

```yaml
CompileFlags:
  Add: [-std=c++17, -Wall, -Wextra, -I../include]
```

`-I../include` is resolved relative to each source file, so it works wherever
the checkout lives. `.clangd` is `clangd`'s own config file and needs no build
step first.

## Building it (optional)

To actually compile and run the program, use CMake:

```sh
cmake -S . -B build
cmake --build build
./build/greeter
```

If you would rather drive `clangd` from real build output, regenerate a
compilation database with `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON` and point
`clangd` at it; a database takes precedence over `.clangd` for the flags it
supplies.

## What to try

- `definition` on `greeter::greet` in `src/main.cpp` → the declaration in
  `include/greeter.hpp`.
- `references` on `struct Person` → its uses in `greeter.cpp` and `main.cpp`.
- `hover` on `Person` → the `/// A person we can greet.` doc comment.
- `diagnostics` on `src/main.cpp` → `Unused variable 'unused'`
  (`-Wunused-variable`, enabled by `-Wall`).
- `format` on any source file → clang-format edits.
