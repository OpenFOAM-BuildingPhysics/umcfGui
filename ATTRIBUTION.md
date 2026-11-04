# Asset attribution

Every asset file in the repository (anything under `assets/` or `docs/images/`) has a row here.
`cargo xtask assets` fails if one is missing. The first column is a path or a glob (`*`).

| Path | Author | Source | Licence | Notes |
|---|---|---|---|---|
| `assets/fonts/Inter-*.ttf` | The Inter Project Authors (Rasmus Andersson) | https://github.com/rsms/inter | OFL-1.1 | The GUI's text font; licence in `assets/fonts/OFL-Inter.txt` |
| `assets/fonts/OFL-Inter.txt` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 | Inter's licence |
| `assets/fonts/JetBrainsMono-Regular.ttf` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono | OFL-1.1 | The GUI's monospace font (parameters, results); licence in `assets/fonts/OFL-JetBrainsMono.txt` |
| `assets/fonts/OFL-JetBrainsMono.txt` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono | OFL-1.1 | JetBrains Mono's licence |
| `docs/images/desktop.png` | umcfGui contributors | Screenshot of `umcf-gui` with a local test case (`--screenshot`) | GPL-3.0-or-later | Own work |
| `docs/images/bc-card.png` | umcfGui contributors | Screenshot of `umcf-gui` with a local test case (`--open bc.set --screenshot`) | GPL-3.0-or-later | Own work |
