# umcfGui

An open-source GUI for [urbanMicroclimateFoam](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam):
set up, run and inspect urban microclimate simulations. Pure Rust, GPL-3.0-or-later.

**Documentation:** https://openfoam-buildingphysics.github.io/umcfGui/

umcfGui is in early development (see [ROADMAP.md](ROADMAP.md)). The libraries that read and
edit OpenFOAM-8 and OpenFOAM-12 cases exist, and so do the command line `umcf-cli` and a first
desktop app `umcf-gui`; meshing set-up and running the solver are still to come.

```sh
git clone --recurse-submodules https://github.com/OpenFOAM-BuildingPhysics/umcfGui.git
cd umcfGui
cargo build --release
cargo xtask ci     # every quality gate
```

Contributors and AI agents: read [AGENTS.md](AGENTS.md) first.
