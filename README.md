# Dwarf Fortress AI Companion

An open-source conversational companion for Dwarf Fortress. The project aims to observe a live fortress through DFHack and provide an AI with useful game context, while leaving control of the game to the player.

## Development environment (Windows)

The companion is written in Rust, with CMake as the build entry point and Cargo managing Rust dependencies. On Windows, use the MSVC Rust target.

Required tools are Git, CMake 3.24 or later, Rust stable via [rustup](https://rustup.rs/) with the `x86_64-pc-windows-msvc` target, and the MSVC linker plus Windows SDK (available from Visual Studio Build Tools). Run the commands below from a Visual Studio Developer PowerShell or another shell where the MSVC build tools are available.

The service uses gRPC over HTTP/2 at `127.0.0.1:50051` and HTTP at `127.0.0.1:3000`. Cargo provisions a host `protoc` through the `protoc-bin-vendored` build dependency, so no machine-wide compiler install is required for Rust code generation. The optional DFHack Lua experiment submits events to `POST /api/plugin/events` as JSON over loopback HTTP; it does not require protobuf support in Lua.

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

CMake invokes Cargo for compilation and tests. Cargo downloads dependencies on the first build, so network access is required. The CMake build directory contains Cargo outputs; commit `Cargo.lock` to keep application builds reproducible.

The `dfhack-remote` crate is used separately for queries to DFHack's remote API; it is not the plugin-to-Companion event RPC. The worker currently uses a mock AI response. Plugin events and web chat share a bounded queue of 50 items; if full, the gRPC/HTTP response asks the plugin script to pause the game and the UI receives a notification.

## Testing Lua-to-Companion communication

The DFHack Lua environment in the tested 53.16-r2 installation includes JSON support (`json`), DFHack's TCP socket wrapper (`plugins.luasocket`), and the `eventful` plugin. `scripts/dfcompanion.lua` uses those modules to send HTTP/1.1 JSON requests and read queue responses. It explicitly switches the socket to blocking mode after connecting because DFHack creates new TCP clients as nonblocking; socket timeouts keep each exchange bounded. When enabled, the script subscribes to `eventful`'s `REPORT` event, looks up report text, converts it from DF's CP437 encoding to UTF-8, and labels announcements separately from other reports. The DFHack documentation warns that reports occur more frequently than announcements, so this experiment may send a high volume of events. Forwarding is synchronous; on a forwarding failure the script disables itself and reports the error, and a full queue pauses the game.

On each startup, DFCompanion generates a fresh cryptographically random 256-bit bearer token. Open the local web page, then copy the displayed `dfcompanion <token>` command and paste it into the DFHack console. The script receives the token as its argument; it is not saved to disk. The page is marked `no-store` so browsers do not cache the token-bearing HTML.

Copy `scripts\dfcompanion.lua` to the installed DFHack `hack\scripts` folder, start DFCompanion, and run the copied command in DFHack with a fortress site loaded. The command enables the script and submits a connection-test event. Run `disable dfcompanion` to disable it, or `enable dfcompanion` to re-enable it during the current DFHack session. `dfcompanion status` reports its in-memory enablement state. The state is not persisted and resets when the map unloads or DFHack exits. If the queue is full, the endpoint returns an explicit queue-full response and the Lua script pauses the game. A new token is generated after each service restart, so copy the current command again.

## Running the development service

After a Debug build, start the service from the repository root:

```powershell
.\build\cargo\debug\dfcompanion.exe
```

Open `http://127.0.0.1:3000/` for the local chat/notification page. The gRPC endpoint listens on `127.0.0.1:50051`. This first code slice implements simulation-event submission only; the planned site snapshot RPC procedures are not implemented yet. AI responses are mocked, the Lua script only submits a manual test event, and local gRPC calls do not yet authenticate clients.
