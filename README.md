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

The DFHack Lua environment in the tested 53.16-r2 installation includes JSON support (`json`) and DFHack's TCP socket wrapper (`plugins.luasocket`), but not a ready-made HTTP client. `scripts/dfcompanion-lua-test-event.lua` uses those modules to send one small HTTP/1.1 JSON request and read the queue response. It explicitly switches the socket to blocking mode after connecting because DFHack creates new TCP clients as nonblocking; socket timeouts keep the exchange bounded. It is a manual transport test, not an event subscription.

The endpoint requires a shared bearer token of at least 32 bytes and rejects any request containing an `Origin` header. Generate a token in PowerShell before starting DFCompanion, then copy the printed token into `PLUGIN_TOKEN` near the top of the Lua script that you install into DFHack:

```powershell
$bytes = New-Object byte[] 32
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
$rng.GetBytes($bytes)
$token = [BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant()
$rng.Dispose()
$token
$env:DFCOMPANION_PLUGIN_TOKEN = $token
.\build\cargo\debug\dfcompanion.exe
```

Copy `scripts\dfcompanion-lua-test-event.lua` to the installed DFHack `hack\scripts` folder, replace its placeholder token with the same generated value, and run `dfcompanion-lua-test-event` in DFHack with a fortress site loaded. A successful response confirms the event was accepted into the Companion queue. If the queue is full, the endpoint returns an explicit queue-full response and the Lua script pauses the game. If `DFCOMPANION_PLUGIN_TOKEN` is unset, the service keeps running but rejects Lua event submissions.

## Running the development service

After a Debug build, start the service from the repository root:

```powershell
.\build\cargo\debug\dfcompanion.exe
```

Open `http://127.0.0.1:3000/` for the local chat/notification page. The gRPC endpoint listens on `127.0.0.1:50051`. This first code slice implements simulation-event submission only; the planned site snapshot RPC procedures are not implemented yet. AI responses are mocked, the Lua script only submits a manual test event, and local gRPC calls do not yet authenticate clients.
