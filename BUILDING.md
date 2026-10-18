# Building DFCompanion

This guide covers developer setup and local builds. User-oriented setup, Lua usage, AI-provider plans, and model suggestions are in [README.md](./README.md).

## Windows prerequisites

The companion service is written in Rust, with CMake as the build entry point and Cargo managing Rust dependencies. The tested setup uses the MSVC Rust target.

Install:

- Git
- CMake 3.24 or later
- Rust stable through [rustup](https://rustup.rs/) with the `x86_64-pc-windows-msvc` target
- MSVC linker and Windows SDK, available through Visual Studio Build Tools
- Dwarf Fortress and DFHack (tested with Dwarf Fortress 53.16 and DFHack 53.16-r2)

Run commands from a Visual Studio Developer PowerShell or another shell where the MSVC build tools are available.

Cargo provisions a host `protoc` through the `protoc-bin-vendored` build dependency; no global protobuf compiler installation is needed for the Rust build.

## Configure and build

Configure CMake with the path to the DFHack installation root—the directory containing `hack\scripts`:

```powershell
cmake -S . -B build -DDFHACK_INSTALL_DIR="E:/SteamLibrary/steamapps/common/DFHack"
```

CMake stores the directory in the local `build` cache, so it need not be repeated. To change installations, rerun configuration with a new path.

Build the service:

```powershell
cmake --build build --config Debug
```

Build the service and install/update the Lua script in DFHack:

```powershell
cmake --build build --config Debug --target install-dfhack-script
```

The install target builds DFCompanion and copies `scripts/dfcompanion.lua` to `${DFHACK_INSTALL_DIR}/hack/scripts` only when the contents differ. It fails with a configuration hint if the setting is empty or the expected scripts directory does not exist.

For a release build:

```powershell
cmake --build build --config Release
```

The first Cargo build downloads dependencies, so network access is required. CMake places Cargo outputs under `build/cargo`. `Cargo.lock` is committed for reproducible application dependency resolution.

## Tests

Run the Rust tests through CTest:

```powershell
ctest --test-dir build -C Debug --output-on-failure
```

## Run the service manually

After a Debug build:

```powershell
.\build\cargo\debug\dfcompanion.exe
```

The HTTP UI and Lua event endpoint listen on `127.0.0.1:3000`; gRPC listens on `127.0.0.1:50051`. The service generates a fresh bearer token on startup and displays the DFHack command containing it on the web page.

## VS Code

The workspace includes a **Build and Install DFCompanion (Debug)** task. After configuring CMake with `DFHACK_INSTALL_DIR`, press **F5** with **Debug DFCompanion** selected. The pre-launch task builds the service and installs the Lua script; the debugger then starts the service and VS Code opens the web UI in the default browser when it detects the ready message.

The launch configuration uses the CodeLLDB debugger. Rust Analyzer alone provides language support but does not provide native debugging.

## Architecture note

The `dfhack-remote` crate is used separately for queries to DFHack's remote API; it is not the Lua-to-Companion event transport. Plugin/Lua events and web chat share a bounded queue of 50 items. The current Companion worker returns mock responses.
