# Fields and results

The solver writes its results as field files in time directories: `3600/air/T`, `3600/buildings/Ts`,
often gzip-compressed (`writeCompression on`) and, for big runs, in binary. umcfGui reads them all.

## What is read

| | |
|---|---|
| Classes | `vol…Field`, `surface…Field`, `point…Field` and anything else with `nonuniform List<T>` data |
| Formats | ASCII and binary, any `arch` (`LSB`/`MSB`, 32- or 64-bit labels and scalars) |
| Compression | `.gz` files are read transparently; `T` is found as `T` or `T.gz` |
| Value types | `scalar`, `vector`, `sphericalTensor`, `symmTensor`, `tensor`, `label` |
| List forms | `N (…)`, `N{value}` (all elements equal), `0()`, and untyped `nonuniform N (…)` |
| Directories | time directories sorted by time (`0`, `0.5`, `3600`, `1e+06`), `processorN`, collated `processorsN` |

The small parts of a field file (header, `dimensions`, every patch of `boundaryField` with its
type and parameters) keep their full structure, the same way as [dictionaries](dictionaries.md).
A patch value such as `$internalField` is reported as written.

## Checked against the mesh and against OpenFOAM

- Every internal field of a volume field has as many values as the mesh has cells (`nCells` in
  the mesh's `owner` header), and every patch `value` list as many as the patch has faces
  (`nFaces` in `boundary`). This holds for every field in the test projects, OpenFOAM-8 and
  OpenFOAM-12.
- Cases converted to binary by OpenFOAM-8's and OpenFOAM-12's own `foamFormatConvert` read back
  bit for bit the same as their ASCII originals.

## Safety and speed

Field files are treated as hostile input. A list that claims two billion elements but holds
two, binary data that ends early, a gzip "bomb" or a `nan` from a diverged run produce an error
or a value, never a crash or a runaway allocation.

Reading runs at about 370 MB/s of field text and decompression at about 235 MB/s, on one core.
The largest file of a real project time step (597 MB of `U`, 16 million vectors) takes about
four seconds.

## Setting boundary conditions

`bc.set` sets the condition of one or more patches in a field of the start time (`0/air/U`),
in the desktop app's [condition card](desktop.md#boundary-conditions) or as a command:

```sh
umcf-cli run --case my-case \
  --cmd 'bc.set={"field": "T", "type": "fixedValue", "value": 290, "patches": [{"region": "air", "patch": "north"}]}' \
  --cmd 'case.save={}'
```

- A patch whose own entry has that type already **keeps its entry**: the value and entries given
  are set in it, and its other keys, comments and `#include`s stay. So does an entry whose type
  comes from an `#include` or a `$macro` in it: the type and entries are set after what it brings
  in, and the `defaults` for keys it doesn't show. Keys given as `defaults` otherwise go into new
  entries only: a patch keeping its entry keeps its own (its value, say). Otherwise the patch gets a new entry, `{ type …; <entries>; value …; }`, in place of its own one, or after the others when it
  had none (a condition from a group or a pattern then still applies to the other patches).
- **Nothing else in the file changes, byte for byte**: not the other entries, the comments, the
  layout nor the data lists, ASCII or binary. A file that wouldn't come back unchanged without
  an edit is refused rather than rewritten. Every field file of the test projects comes back
  byte for byte. A `.gz` file is compressed again: its text is kept byte for byte, its
  compressed bytes generally not.
- A **uniform value must fit the field**: one number for a scalar field, three for a vector
  (`uniform (2 0 0)`), and so on, for `value` and the `…Value` entries (`inletValue`); a mismatch
  is refused. A field of another class (`volVector2DField`) is not checked.
- A **new entry replacing one that took text from an `#include` or a `$macro`** is reported
  (`bc.replaced`): that text no longer applies to the patch. A value in the old entry that
  names a variable (`value $internalField;`) is not reported.
- When `boundaryField` itself takes entries from an `#include` or a `$macro`, the command warns
  (`bc.indirect`): one of them may set the same patches after the entries set here.
- A field whose `boundaryField` comes from elsewhere (`boundaryField $common;`, an `#include`)
  is not edited: the condition is in that other file.
- The edit is one step in the undo history, and is written by `case.save` after a backup. An
  undo after the save is a change to save again: saving writes the file as it was.
- Text OpenFOAM reads as a list's data while the file is edited (`__umcfList…`) and comments in
  a value are refused.
- The command returns the [case check](checks.md)'s findings for those patches and that field: a
  type the solver doesn't have for the case's OpenFOAM version, or a constraint patch (`empty`,
  `cyclic`) given another type.

