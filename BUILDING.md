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

The install target builds DFCompanion and copies `scripts/*.lua` to `${DFHACK_INSTALL_DIR}/hack/scripts` and `lua/**/*.lua` to `${DFHACK_INSTALL_DIR}/hack/lua`, only when the contents differ. The latter holds `rpc.dfcompanion`, the module the service calls through DFHack's `RunLua` RPC (DFHack only permits modules named `rpc.*`). The target fails with a configuration hint if the setting is empty or the expected scripts directory does not exist. A running game caches modules it has already loaded, so after changing a `lua/` file, restart the game or run `lua package.loaded['rpc.dfcompanion']=nil` in the DFHack console.

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

### Live-game tests

`tests/live_game_tests.rs` talks to a real game through DFHack's remote interface. These tests are marked `#[ignore]`, so CTest and plain `cargo test` skip them. They are read-only and do not change the game.

Before running them, start Dwarf Fortress with DFHack, load a fortress (a game with no fort loaded may make some tests fail), and make sure DFHack's remote server is listening on its default port. Then run them with Cargo from the repository root:

```powershell
# All live tests
cargo test --test live_game_tests -- --ignored

# One test, with its printed output (e.g. tool results as the AI sees them)
cargo test --test live_game_tests ai_tools_end_to_end -- --ignored --nocapture
```

To check whether the game is running first: `Get-Process -Name "Dwarf Fortress"`.

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

The `dfhack-remote` crate is used separately for queries to DFHack's remote API; it is not the Lua-to-Companion event transport. Plugin/Lua events and web chat share a bounded queue of 50 items. Reports are buffered for the next player-triggered AI turn; they do not trigger API calls on their own. Provider adapters use HTTPS directly, while provider/model settings are stored in per-user local application data and API keys use the operating system credential vault.
