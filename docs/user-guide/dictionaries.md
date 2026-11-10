# Dictionaries

OpenFOAM cases are folders of text dictionaries, and people edit them by hand. umcfGui treats
your files as yours.

## Nothing is lost

Reading a dictionary and writing it back gives the same bytes. umcfGui keeps:

- comments, blank lines, alignment and tabs,
- `#include`, `#includeEtc`, `#includeFunc` (also the multi-line OpenFOAM-12 form),
  `#remove`, `#inputMode`,
- `$macros`, `${scoped.macros}` and `$:defaults`,
- `#calc` expressions and `#{ … #}` code blocks,
- quoted and regex keys such as `"(wall|ground).*"`, and keys such as `div(phi,U)`,
- keys and types umcfGui doesn't know,
- bare data files such as `constant/IDN`, `sunPosVector` or time tables.

umcfGui doesn't expand `#include` or `$macros` in place: they stay as you wrote them.

## Edits touch one entry

Changing a value replaces only that value. Its key, comment and column stay:

```c++
div(phi,U)      bounded Gauss linearUpwind grad(U); // momentum
```

becomes

```c++
div(phi,U)      bounded Gauss upwind; // momentum
```

A new entry is laid out like its neighbours, with the same indentation, the same blank-line
spacing and its value at the column of the value before it (aligned with spaces; one space
when that one has one; else OpenFOAM's usual 16); in a dictionary written all on one line
(`{ name soil; rho 1600; }`) it goes on that line, but never after a `//` comment. A new sub-dictionary gets the usual OpenFOAM
layout, braces on their own lines (inside a dictionary written on one line it is laid out less
neatly, but reads the same). Removing an entry also removes the comments on its line after it
(a `/* … */` one whole, over lines too) and keeps those of the entry before it; where two
entries share a line, a comment between them is the first one's. New lines end in `\n`, also in
a file whose lines end in `\r\n` (OpenFOAM reads both).

## Problems are reported, not fatal

Some files OpenFOAM tolerates are not quite right: an entry without its `;`, a stray `;` or
`}`. umcfGui keeps them, so the file still writes back unchanged, and lists them as problems.
Real syntax errors (an unclosed bracket or string) come with a line and column.

## How it is checked

Every dictionary in the OpenFOAM-8 and OpenFOAM-12 tutorials and `etc/` directories (8,795 files)
and in real UMCF projects round-trips byte for byte, and random and corrupted input never makes
the parser crash. See [Contributing](../contributing.md#tests-against-real-cases).
