---
applyTo: "lua/**"
---

# Using dfhack-run

An MCP server is defined named "dfhack" to allow you to run dfhack-run commands
through the MCP protocol. This enables you to execute Lua fragments, reload
scripts, and interact with the game without directly modifying the running
instance. Arguments are generally interpreted as if typed directly into the
DFHack console in the game. One exception is the command `lua`. (In the console
the user must give it as `:lua`.)

The `lua` argument to `dfhack_run` executes the following string as a Lua fragment
in the game's Lua environment. The fragment has access to all global variables
and functions available in the game's Lua environment, including `df`, `dfhack`,
and any installed modules.

`dfhack_run` only works when a game is running with a fortress loaded. It does
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
local job = nil
local i_wanted = 2
for i, item in utils.listpairs(df.global.world.jobs.list) do
    if i == i_wanted then
        job = item
        break
    end
end
for i, element in ipairs(job.job_items.elements) do
    print('element', i, element.quantity)
end
for i, ref in ipairs(job.items) do
    print('ref', i, ref.job_item_idx)
end
```

Important: DFHack's vectors are 0-based because they originate from C++ code,
not 1-based as is more common in Lua. When matching a vector index against a
stored index field (such as `job_item_idx`), do not subtract one. Confirm the
base with a probe when in doubt.  

Important: df.global.world.jobs.list and df.global.world.projectiles.list are
linked lists, not standard Lua tables. Use `utils.listpairs()` to traverse them
rather than `ipairs()`. You also cannot index directly into
`df.global.world.jobs.list` or `df.global.world.projectiles.list` using square
brackets.

# Testing Lua fragments

To test a fragment in isolation without modifying the installed modules, run it
directly using `dfhack_run`'s `lua` argument. Use this to validate an
expression, a field name, or a small algorithm before wiring it into a module. 

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

This copies the files into the DFHack install tree. 

Editing the source files alone changes nothing in the running game. Always
install after editing before testing.


## Reload changed plugin scripts in the running game

To reload a changed plugin script, use the `reload` function. Example:

```text
lua "reload('dfcompanion')"
```

Reloading a plugin script disables it. To re-enable the plugin, use dfhack-run's
built-in enable command::

```text
enable dfcompanion
```

Always re-enable the `dfcompanion`plugin after reloading it.

## Reload changed Lua script modules in the running game

DFHack caches script modules. A running game will keep using the
old version of a module if it's updated while the game is running.

To force a reload of a changed Lua script module, use dfhack-run to execute a
call to reqscript(). This will work in most cases, unless another module has
cached a reference to a function in the reloaded script, or a script location
has changed. Example arguments:

```text
lua "reqscript('dfcompanion.internal.places')"
```


# Verification order

1. Edit the Lua source.
2. Run `cmake --build build --config Debug --target install-dfhack-script`.
3. Reload the changed file using the methods described earlier.
4. Probe the installed module through `dfhack-run`.
5. Run live Rust tests from the repository root with the game running and a
   fortress loaded. `cargo test --test live_game_tests -- --ignored`.
