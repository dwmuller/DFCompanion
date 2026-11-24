---
applyTo: "lua/**"
---

# Running arbitrary Lua in the running game

Use `dfhack-run.exe` from the `hack` directory under the DFHack installation to
execute Lua in the live game. The installation path is defined as
DFHACK_INSTALL_DIR by CMake during the build process. To read it, open
build/CMakeCache.txt and use the value of the line that starts with
`DFHACK_INSTALL_DIR:`, for example
`DFHACK_INSTALL_DIR:PATH=E:/SteamLibrary/steamapps/common/DFHack`. The examples
in this document use `<DFHACK_INSTALL_DIR>` as a placeholder for the actual
installation path.

Invoke it with the `lua` command and pass the script as a single argument:

```text
& "<DFHACK_INSTALL_DIR>\hack\dfhack-run.exe" lua "print(df.global.world.units.active[0].id)"
```

The script runs in the game's Lua environment with full access to `df`, `dfhack`,
and the installed `dfcompanion` modules. Output is printed to the console.

From PowerShell, use a here-string to avoid quoting problems with multi-line
scripts:

```powershell
$lua = @'
local places = require('dfcompanion.internal.places')
print(places.list_workshops('', 'true', 0, 10))
'@
& "<DFHACK_INSTALL_DIR>\hack\dfhack-run.exe" lua $lua
```

`dfhack-run` only works when a game is running with a fortress loaded. It does
not start the game.

# Probing DFHack behavior

Before writing code that depends on a DFHack structure, probe the live data
first. Print the actual fields rather than guessing at names or index bases.

Probe templates:

```lua
-- Print a table's keys and values
local u = df.global.world.units.active[0]
for k, v in pairs(u) do print(k, v) end
```

```lua
-- df.global.world.jobs.list and df.global.world.projectiles.list are unusual
-- in that they're linked lists. Use utils.listpairs() to traverse:
for i, item in utils.listpairs(df.global.world.jobs.list) do
    print(i, item.id, item.job_type)
end
```

```lua
-- Inspect a specific job's requirements and attached items
local job = find_job(some_predicate) -- hypothetical function to locate a job matching some condition
for i, element in ipairs(job.job_items.elements) do
    print('element', i, element.quantity)
end
for i, ref in ipairs(job.items) do
    print('ref', i, ref.job_item_idx)
end
```

Important: DFHack's `ipairs` over vectors yields **0-based** indices. This
differs from standard Lua tables. When matching a vector index against a stored
index field (such as `job_item_idx`), do not subtract one. Confirm the base with
a probe when in doubt.

# Testing Lua fragments

To test a fragment in isolation without modifying the installed modules, run it
directly through `dfhack-run`. Use this to validate an expression, a field
name, or a small algorithm before wiring it into a module.

```powershell
$lua = @'
local total = 0
for _, ref in ipairs(df.global.world.jobs.list[0].items) do
    total = total + 1
end
print('attached refs:', total)
'@
& "<DFHACK_INSTALL_DIR>\hack\dfhack-run.exe" lua $lua
```

Keep fragment tests read-only. Do not call functions that mutate game state
during a probe unless the user has explicitly asked for that behavior.

After a fragment works, move the logic into the module, install it, reload the
module, and re-run the same probe through the installed path to confirm the
packaged version behaves identically.

# Installing updated Lua files

The Lua sources under `lua/` are installed into the DFHack directory by the
`install-dfhack-script` CMake target. After any change to a Lua file, run:

```text
cmake --build build --config Debug --target install-dfhack-script
```

This copies the files into the DFHack install tree. The installed locations are
reported by the target, typically:

- `lua/dfcompanion.lua` -> `<DFHack>/hack/scripts/dfcompanion.lua`
- `lua/dfcompanion/**` -> `<DFHack>/hack/lua/dfcompanion/...`
- `lua/rpc/**` -> `<DFHack>/hack/lua/rpc/...`

Editing the source files alone changes nothing in the running game. Always
install after editing before testing.

# Reloading changed Lua files in the running game

DFHack caches script modules. A running game will keep using the
old version of a module if it's updated while the game is running.

To force a reload of a changed Lua script module, use dfhack-run to execute a
call to reqscript(). This will work in most cases, unless another module has
cached a reference to a function in the reloaded script, or a script location
has changed. Example:

```powershell
& "<DFHACK_INSTALL_DIR>\hack\dfhack-run.exe" lua "reqscript('dfcompanion.internal.places')"
```

To reload a plugin script, use the `reload` function instead. Example:
```powershell
& "<DFHACK_INSTALL_DIR>\hack\dfhack-run.exe" lua "reload('dfcompanion')"
```

# Verification order

1. Edit the Lua source.
2. Run `cmake --build build --config Debug --target install-dfhack-script`.
3. Reload the changed file using the methods described earlier.
4. Probe the installed module through `dfhack-run`.
5. Run the Rust tests only when the edited Lua file is called from Rust code or
   changes a value that Rust reads (for example, an RPC response shape). Run them
   from the repository root:
   `cargo test --test live_game_tests -- --ignored`, with the game running and a
   fortress loaded. For all other Lua-only changes, skip this step.
