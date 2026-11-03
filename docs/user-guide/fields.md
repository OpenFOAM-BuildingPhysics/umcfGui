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
