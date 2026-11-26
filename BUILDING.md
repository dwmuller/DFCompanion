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

Optional: [pnpm](https://pnpm.io/) (for example `winget install pnpm.pnpm`), only to update the vendored web UI libraries; see [Web UI libraries](#web-ui-libraries).

Run commands from a Visual Studio Developer PowerShell or another shell where the MSVC build tools are available.

## Configure and build

Configure CMake with the path to the DFHack installation root—the directory containing `hack\scripts`:

```powershell
cmake -S . -B build -DDFHACK_INSTALL_DIR="E:/SteamLibrary/steamapps/common/DFHack"
```

CMake stores the directory in the local `build` cache, so it need not be repeated. To change installations, rerun configuration with a new path.

### Lua language server configuration

The Lua scripts run inside DFHack and use its globals (`dfhack`, `df`, `mkmodule`, and so on) and its Lua libraries. Configuring CMake with `DFHACK_INSTALL_DIR` also generates `.luarc.json` in the repository root from the template `.luarc.json.in`. This configures [Lua Language Server](https://luals.github.io/) (for example the VS Code Lua extension) to use Lua 5.3, declare the DFHack globals, and resolve `require()` calls against `${DFHACK_INSTALL_DIR}/hack/lua` and `${DFHACK_INSTALL_DIR}/hack/scripts`.

`.luarc.json` contains a machine-specific path and is ignored by git. Edit `.luarc.json.in` to change the shared settings, then rerun the CMake configure step to regenerate the file. If `DFHACK_INSTALL_DIR` is empty, configuration prints a warning and skips generating the file, and the language server will report DFHack globals and modules as undefined.

The language server has no type information for `dfhack.*` or `df.*`, so it does not check those fields.

Build the service:

```powershell
cmake --build build --config Debug
```

Build the service and install/update the Lua script in DFHack:

```powershell
cmake --build build --config Debug --target install-dfhack-script
```

The install target builds DFCompanion and copies `lua/plugin/*.lua` to `${DFHACK_INSTALL_DIR}/hack/scripts` and the remaining `lua/**/*.lua` files to `${DFHACK_INSTALL_DIR}/hack/lua`, only when the contents differ. The latter holds `rpc.dfcompanion`, the module the service calls through DFHack's `RunLua` RPC (DFHack only permits modules named `rpc.*`), plus private `dfcompanion.internal.*` domain modules. The target fails with a configuration hint if the setting is empty or the expected scripts directory does not exist. A running game caches modules it has already loaded, so after changing a `lua/` file, restart the game or clear the facade and all its internal modules in the DFHack console:

```lua
for name in pairs(package.loaded) do
    if name == 'rpc.dfcompanion' or name:match('^dfcompanion%.internal%.') then
        package.loaded[name] = nil
    end
end
```

### Game-state reporting

The service tracks how far the game has loaded—DFHack reachable, world loaded, fortress loaded, and the fortress name—so its AI game tools stay inert until a fortress is ready; this avoids querying fort data while a fortress is loading, which can crash the game. The state is kept current two ways. `lua/plugin/dfcompanion.lua` pushes it to the service on DFHack's world/map load and unload events, and the service also polls DFHack for it (about every 20 seconds), so the state converges even if no push arrives, such as when the service starts while a fortress is already loaded.

The push is registered when the script runs, independently of the `enable dfcompanion` event-forwarding toggle. Once loaded, the script restores the forwarding setting from fortress site data on map load. A fortress with no companion site data remains unconfigured until it contacts the service, then enables forwarding and saves the setting; older site data without an enabled field defaults to enabled. Failed announcements while unconfigured are logged and skipped without pausing. `enable dfcompanion` and `disable dfcompanion` save the setting for the current fortress. The service's lifecycle poll loads the script; to load it at DFHack startup, add `dfcompanion status` to a `dfhack-config/init/dfhack*.init` file (it works with no fortress loaded). When forwarding is enabled, a delivery failure retains the failed report and pauses the game; restarting the service and unpausing retries that report. Pending reports are discarded with a warning on fortress unload rather than sent to a different fortress.

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

## Web UI libraries

The web UI renders conversation text as Markdown with [marked](https://github.com/markedjs/marked) and sanitizes it with [DOMPurify](https://github.com/cure53/DOMPurify). Their minified builds are committed in `static/vendor/` and compiled into the binary, so building the service does not need Node or pnpm. To change versions, edit `package.json` and run `pnpm install`, then `scripts\update-vendor.ps1`, which installs from the lockfile and copies the builds into `static/vendor/`. Commit the lockfile and the vendored files.

## Run the service manually

After a Debug build:

```powershell
.\build\cargo\debug\dfcompanion.exe
```

The HTTP UI and Lua event endpoint listen on `127.0.0.1:3000`.

## VS Code

The workspace includes a **Build and Install DFCompanion (Debug)** task. After configuring CMake with `DFHACK_INSTALL_DIR`, press **F5** with **Debug DFCompanion** selected. The pre-launch task builds the service and installs the Lua script; the debugger then starts the service and VS Code opens the web UI in the default browser when it detects the ready message.

The launch configuration uses the CodeLLDB debugger. Rust Analyzer alone provides language support but does not provide native debugging.

For the Lua scripts, install a Lua language server extension (such as the Lua extension by sumneko). It reads the `.luarc.json` generated by CMake; see [Lua language server configuration](#lua-language-server-configuration).

## Architecture note

The `dfhack-remote` crate is used separately for queries to DFHack's remote API; it is not the Lua-to-Companion event transport. Plugin/Lua events and web chat share a bounded queue of 50 items. Reports are buffered for the next player-triggered AI turn; they do not trigger API calls on their own. Provider adapters use HTTPS directly, while provider/model settings are stored in per-user local application data and API keys use the operating system credential vault.
