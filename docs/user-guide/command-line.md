# Command line

`umcf-cli` works on a case without the GUI. It runs the same commands the GUI will. Build it
with `cargo build --release -p umcf-cli`; the program is `target/release/umcf-cli`.

## Checking a case

The examples below write `umcf-cli` for short: put `target/release` on your `PATH`, or give
the full path.

```sh
target/release/umcf-cli check path/to/case
```

This opens the case and prints every problem the [case check](checks.md) finds, errors first,
then a summary:

```text
error    workflow.all-regions         Allrun: line 46: `reconstructPar -allRegions` reads the region list from controlDict `regionSolvers`, …
warning  fv.options-ignored           system/air/fvOptions [air]: OpenFOAM-12 reads fvOptions only when there is no fvConstraints: …
1 error, 3 warnings, 2 notes
```

Each line gives the level, the code, then the file and, in brackets, the region, patch and
field.

| Option | Does |
|---|---|
| `--json` | Prints one JSON object: `errors`, `warnings`, `infos` (the counts) and `diagnostics`, a list of `{level, code, path, region, patch, field, message}` |
| `--no-fields` | Doesn't read the field files. Opening is faster, but boundary conditions are not checked: each field file gets a note saying so. A region with no field files at all is still an error (`region.no-fields`), so the status can still be 1 |

The exit status is 0 when there are no errors (warnings are allowed), 1 when there are, and 2
when the case could not be checked (it isn't a directory, or doesn't open). In a script:
`umcf-cli check case || exit 1`.

## Checking a mesh

```sh
umcf-cli mesh path/to/case [--region air] [--json]
```

This reads each region's mesh (or only `--region`'s) and runs the
[mesh quality checks](meshes.md#mesh-quality). It prints the problems found, then one line per
region:

```text
warning  mesh.aspect-ratio            constant/buildings/polyMesh [buildings]: 2880 cells have an aspect ratio above 1000 (max 2046): cells 0, 1, 2, …
air: 25588 cells, non-orthogonality max 69.2°, skewness max 0.82, 0 errors, 0 warnings
```

`--json` prints, for each region:
- `quality`: the numbers (cells, faces, volumes, face areas, aspect ratio, non-orthogonality,
  skewness, openness), with the first 1000 face or cell labels of each problem and their count;
- `checkmesh`: what `checkMesh` left in the case, its `logs` (verdict, failed checks, remarks)
  and `sets` (each `-writeSets` set with its count, first 1000 labels and whether it is stale);
- `diagnostics`: the problems as above, the native checks' and `checkMesh`'s.

The top-level `diagnostics` holds notes about the case's `checkMesh` logs as a whole, such as a
log for no region of the case.

The exit status is 0 when there are no errors, 1 when there are, and 2 when the case can't be
opened. A mesh that can't be read is an error of its region (`mesh.unreadable`); the other
regions are still checked.

## Running commands

```sh
umcf-cli run --case path/to/case --cmd 'control.set={"endTime": 7200}' --cmd case.save
```

This runs the commands in order and stops at the first that fails. Each command is given as
`<id>=<parameters>`, with the parameters as a JSON object; a command without parameters is just
its id. `--case` opens the case first (it is the command `case.open`).

Edits change the case in memory. Only `case.save` writes them to disk:

- It writes only the files that changed, and edits each one in place (comments, layout and
  `#include`s stay as they were).
- The first time a file is saved, its original is copied under
  `.umcfgui/backup/<session>/` in the case.
- If a file changed on disk since the case was opened, nothing is saved. Each file is checked
  once more right before it is replaced; a file changed in that moment stops the save, and the
  error names the files already written.

A run that ends with unsaved edits says so.

When a command fails, the case in memory is left as it was before that command: `control.set`
with several entries sets all of them or none. Files already written are not: a `case.save`
that stops part way leaves the files it wrote saved (the error names them).

| Option | Does |
|---|---|
| `--cmd '<id>=<json>'` | One command; repeat it for more |
| `--cmds <file>` | Commands from a file, one JSON object per line: `{"command": "control.set", "params": {"endTime": 7200}}`. Blank lines and lines starting with `#` are skipped |
| `--journal <file>` | When the run ends, writes the edits made to the case open at that point, in the same format. Opening a case is not recorded, and starts a new journal |
| `--json` | One JSON line per command: `{command, ok: true, message, data}`, or `{command, ok: false, message}` for the one that failed |

A journal from `--journal` holds the edits of one case, and runs again with `--cmds` on
whichever case is open: `umcf-cli run --case copy --cmds journal.jsonl` makes the same edits
to `copy`. A run that opens a second case journals only the edits made after that.

The exit status is 0 when every command ran, 1 when one failed or the journal could not be
written (the commands before may have run, and `case.save` may have written files), and 2 for
bad arguments.

## The commands

`umcf-cli commands` lists them with their parameters (`--json` for programs).

| Command | Does |
|---|---|
| `case.open` | Opens a case: `path` (required), `fields` (`false` skips reading field files), `discard` (`true` opens even if the open case has unsaved changes) |
| `case.save` | Writes the changed dictionaries, after a backup of each |
| `case.info` | The case: OpenFOAM version, kind, regions with their patch and field counts, `processor*` directories, unsaved files, what didn't read |
| `case.check` | The [case check](checks.md) |
| `mesh.check` | The [mesh quality checks](meshes.md#mesh-quality): `region` (default: every region with a mesh) |
| `control.set` | Sets `system/controlDict` entries, by name: `{"endTime": 7200, "writeInterval": 600}` |
| `dict.set` | Sets one entry of any dictionary the case loaded: `file`, `path` (`"PIMPLE/nOuterCorrectors"`), `value`. Parent dictionaries that are missing are added |
| `dict.remove` | Removes one entry: `file`, `path` |
| `edit.undo`, `edit.redo` | Step back and forward through the edits (up to 100) |

Values are given as JSON:

- a number or `true`/`false` is written as it is;
- an array becomes an OpenFOAM list: `[0, 0, -9.81]` is `(0 0 -9.81)`;
- a string is OpenFOAM text, written as given: `"uniform 300"`, `"Gauss linear"`, or a
  dictionary such as `"{ type fixedValue; value uniform 300; }"`.

A value can only change its own entry. Text that would end the entry early is refused, such as
a `;` or anything after a dictionary's closing `}`. A comment in a value is dropped. Keys are
plain OpenFOAM words (`endTime`, `div(phi,U)`) or patterns, given without their quotes
(`(U|k)` reaches `"(U|k)"`, and a new one starting with `(` is written quoted; a new
pattern such as `"U.*"` can't be added yet, as it would be written unquoted, as a plain word): directives (`#include`), macros (`$x`) and keys
with unbalanced brackets are refused. So are unknown parameter names, which catches typos.
