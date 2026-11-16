## Can Companion itself reliably query game state through dfhack-mcp?

Short answer: **yes, technically** — but I'd recommend *not* making it the sole mechanism for the safety-critical load-gate we discussed earlier. Here's the reasoning:

- MCP is transport/language-agnostic. If DFCompanion embeds a Rust MCP *client* (the official `rmcp` crate, from the `modelcontextprotocol` org, is the natural fit since this project is already async/tokio), it can call `dfhack-mcp`'s `game_state()` tool directly from its own code — not just route it to the AI. That tool is explicitly documented as safe at any screen and "cheap enough to poll."
- **But** this makes a core safety mechanism depend on an optional external Node process being installed, running, and healthy. If `dfhack-mcp` isn't installed, crashed, or is mid-restart, Companion would lose its own "is a fortress loaded" signal at exactly the moment it matters most.
- DFHack's remote RPC server does accept multiple concurrent TCP clients (that's how `dfhack-mcp`, `dfhack-remote`, and the Lua `RunLua` RPC can all run simultaneously today) — so there's no protocol reason Companion and `dfhack-mcp` can't both talk to DFHack at once. I'd still verify this live rather than assume it, as step 0 below.

**My recommendation:** keep a minimal, dependency-free lifecycle check in Companion's own code (the `isWorldLoaded`/`isMapLoaded` Lua push we discussed) as the *authoritative* signal for the fort-loading crash guard, and treat `dfhack-mcp`'s `game_state()` as a convenience/cross-check once it's wired up anyway — not a replacement for the core safety path. Everything else (fort details, legends, units, etc.) is a great candidate to delegate entirely to `dfhack-mcp` rather than duplicating in `game_state.rs`.

## Plan: lightweight incorporation of `dfhack-mcp`

**Phase 0 — Validate before building anything**
- Manually run `npx -y dfhack-mcp@<pinned-version>` against your already-running DF+DFHack, confirm it connects alongside `dfhack_remote`/Lua RPC without conflict, and exercise `game_state()` plus a couple of fort-data sensors by hand (an MCP inspector CLI or a throwaway script). Pin the exact version you validate against — this answers the "specific version" ask directly.

**Phase 1 — Deployment/build**
- Add Node.js as a new, clearly documented *optional* prerequisite in `BUILDING.md` (separate from the existing optional pnpm note, which is for vendor libs only) — only required if the player wants the `dfhack-mcp` tools enabled.
- Store the pinned package version as a constant (e.g. in `ai_providers.rs`-adjacent config or a new small module); Companion spawns `npx -y dfhack-mcp@<version>` (or a configurable path, for a global install) rather than vendoring Node code into the repo.

**Phase 2 — Rust MCP client**
- Add `rmcp` (evaluate its maturity/API stability first — it's from the official org but still young) as a dependency.
- New module (e.g. `src/mcp_client.rs`): spawns the child process over stdio, performs the MCP `initialize` handshake, lists tools, and exposes a typed async `call_tool(name, args) -> Result<Value, String>`.

**Phase 3 — Process supervision**
- Lifecycle: spawn at startup (or lazily on first use) behind an enable/disable setting (default **off**, since it's an optional external dependency and your project treats unreleased features conservatively); detect readiness via the handshake; detect crash/exit and restart with backoff; clean shutdown (kill child) on Companion exit; a `Notify` on failure/unavailability so the player sees it in chat, same pattern as other subsystem warnings.

**Phase 4 — Expose its tools to the AI**
- `ai_tools.rs`'s current `registry()`/`ToolSpec`/`Handler` is synchronous and `GameState`-only by design (per its own doc comment) — I'd leave that untouched and add a **parallel, additive path**: `ai_tools::definitions()` merges in `Tool`s discovered from the MCP client's tool list, and `execute_named` checks the local registry first, then falls back to an async MCP call by name. This avoids reshaping the existing synchronous tool machinery for the handful of external tools.

**Phase 5 — Decide what's exposed (fog-of-war / player-knowledge fit)**
- `dfhack-mcp`'s sensors don't follow your project's "AI should know only what the player knows" principle the way `known_creatures` does (its `reveal_hidden` flag only gates undiscovered *tiles*, not e.g. legends/historical-figure lookups, which read world data a player may never have learned in-game). I'd start with an **explicit allowlist** of its tools to expose (e.g. `fort_status`, `stocks`, `threats`, `jobs_and_labor`, `military`, `geology`, map tools) rather than exposing all 37+ automatically, and leave actuators disabled entirely (`DFHACK_MCP_ACTUATORS` unset) since MVP scope is read-only.

**Phase 6 — Internal (non-AI) use**
- Add a thin `GameState`-adjacent helper that calls `game_state()` via the MCP client for status/diagnostics display, but — per the recommendation above — keep it as a secondary signal, not the load-gate's source of truth.

**Phase 7 — Docs**
- `BUILDING.md`: Node prerequisite, enable/disable setting, version pin.
- `.github/copilot-instructions.md`: note the new optional external dependency and where its client code lives.
- `notes/TODO.md`: replace/refine the `? Refactor AI tools as an MCP service` line — this plan is closer to "consume an MCP service" than "become one," worth distinguishing. Also a pointer for the `crystal-ball` idea as a likely-later second MCP service, same mechanism.

## Open questions before I lock this into a file-by-file plan

1. **Enabled by default, or opt-in?** I lean opt-in (off by default) given the new Node dependency and that it's third-party code you don't control.
2. **Version to pin** — do you want to pick it now (I can check `dfhack-mcp`'s npm releases for the latest tagged version), or leave that for the implementing agent to decide during Phase 0 validation?
3. **Tool allowlist** — happy to propose a concrete starter list (Phase 5) once you confirm direction, or leave that to the next agent as part of the plan.
4. Should I also sketch how `crystal-ball` would layer in later (same MCP-client mechanism, second spawned process), or keep this plan scoped to `dfhack-mcp` only for now, as you said?