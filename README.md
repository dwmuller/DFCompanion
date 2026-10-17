# Dwarf Fortress AI Companion

An open-source conversational companion for Dwarf Fortress. The project aims to observe a live fortress through DFHack and provide an AI with useful game context, while leaving control of the game to the player.

## Development environment (Windows)

The companion is written in Rust, with CMake as the build entry point and Cargo managing Rust dependencies. On Windows, use the MSVC Rust target.

Required tools are Git, CMake 3.24 or later, Rust stable via [rustup](https://rustup.rs/) with the `x86_64-pc-windows-msvc` target, and the MSVC linker plus Windows SDK (available from Visual Studio Build Tools). Run the commands below from a Visual Studio Developer PowerShell or another shell where the MSVC build tools are available.

No separate `protoc` installation is needed at this stage. The planned `dfhack-remote` Rust crate integration handles its own protobuf code generation; a project-owned `protoc` may be needed later if we add a C++ DFHack plugin or define and compile our own protobuf schemas. `dfhack-remote` is temporarily not in the manifest because its current published release has an unresolvable yanked transitive dependency; add it when that dependency can be resolved cleanly. The current CMake project declares no native languages, so it does not require a C++ compiler to be configured by CMake; the MSVC linker and SDK are still needed by Rust's Windows MSVC target.

## Building

Configure and build from the repository root:

```powershell
cmake -S . -B build
cmake --build build --config Debug
```

For a release build, use:

```powershell
cmake --build build --config Release
```

Run the Rust tests through CTest:

```powershell
ctest --test-dir build -C Debug --output-on-failure
```

CMake invokes Cargo for compilation and tests. Cargo downloads dependencies on the first build, so network access is required. The CMake build directory contains Cargo outputs; commit `Cargo.lock` once the dependency graph can be resolved to keep application builds reproducible.

The current scaffold intentionally does not depend on `dfhack-remote`, so it can be built independently while that crate's published release has a clean dependency resolution issue: its transitive `zip ^1.1.1` build dependency has only yanked matching releases on crates.io. Do not work around this by silently pinning a yanked release. Revisit the dependency when upstream provides a cleanly resolvable release or another suitable integration approach is selected.
