# Dwarf Fortress AI Companion: Project Requirements Summary

## Vision

Build an open-source conversational companion for Dwarf Fortress that can
observe a live fortress through DFHack and give an AI enough context to answer
player questions intelligently.

The goal is assistance and understanding, not autonomous play. The player
remains responsible for interacting with and controlling Dwarf Fortress. The
only effect the companion can have on the game is to pause it.

The companion will not expose information to the AI that is not accessible to
the user, although it may receive information organized in ways that differ from
the UI for the convenience of the AI.

The scope is fortress mode at an active site; other game modes are not supported.

## Primary actors and boundaries

- **Player:** interacts directly with Dwarf Fortress and converses with the AI
  through the companion.
- **AI:** converses with the player and receives/querys game observations
  through the companion.
- **Game environment:** Dwarf Fortress is the authoritative simulation; DFHack
  is the observation interface into it.
- **Companion system:** mediates AI/game interaction and may retain information
  that cannot reliably be reconstructed from current game state.

Direct player/game interaction is outside the companion's scope.

## MVP objectives
1. Receive selected fortress simulation events from a DFCompanion DFHack plugin
   via a Web API in DFCompanion.
2. Put plugin events and player chat into one Companion input queue, distinguished by category.
3. Maintain a conversational UI in which the AI can combine player questions with live game context and query current game state through bounded, read-only tools.
4. Process events asynchronously, batch them on a short idle timeout or queue-pressure threshold, and offer each batch to the AI without requiring a user-visible response for every batch.
5. Ask the plugin to pause the game if the bounded event queue fills.
6. Persist Companion-owned, site-specific context alongside the corresponding game site so it can be restored on another machine.
7. Keep installation and deployment practical on both the gaming desktop and laptop.


## Major components

- **Plugin:** A DFHack plugin named dfcompanion. It is responsible for sending
  game reports and state changes to the DFCompanion program via a Web API.
- **Plugin RPC interface:** a gRPC service hosted in the DFCompanion process. The DFHack plugin is its client and submits simulation events and exchanges site-state snapshots during site save/load callbacks.
- **Shared input queue:** a bounded in-memory queue of capacity 50 (a code constant, not initially a user setting). Plugin events carry a category; chat messages use the reserved `user_input` category.
- **Companion worker:** consumes queued events, may batch them, applies AI API rate limits, and presents resulting messages in the shared conversation stream. The user may also prompt the AI directly.
- **HTTP/UI service:** serves a browser interface with a chat prompt, one notification/conversation stream for AI output, and configuration controls. Server-Sent Events (SSE) deliver updates to the page.
- **DFHack access:** the AI may request current game state only through bounded, read-only Companion tools backed by an application-allowlisted set of DFHack remote API calls or plugin-provided procedures. The AI never receives direct access to the remote interface. Checks game/DFHack version compatibility before any tool query; warn the user or request confirmation if versions differ.

The Companion worker supports OpenAI Chat Completions, OpenAI Responses, and
Anthropic through the provider-neutral `genai` Rust library. Users can create
named configurations with an interaction method, URL, model ID, API key, and
editable system text, then select one for chat. A static built-in provider
catalog offers editable setup presets; the provider choice is not persisted in a
configuration. Configuration metadata is stored per-user locally and each API
key is stored separately in the OS credential vault. New installations start
without configurations; the previous single-provider settings are not migrated.
Google Gemini is intentionally unsupported because its terms do not permit the
project's intended use. Player chat is submitted to the same queue as simulation
events. Reports are accumulated as context and optionally  trigger independent
model calls. One sequential AI turn is made per player message.

## Game event policy

When an error occurs enqueing an event (e.g. due to a full queue), the RPC
response tells the plugin that a pause is requested and that the incoming event
was not accepted. The plugin is responsible for pausing the game. DFCompanion
also publishes a queue-full notification to the UI. Queue capacity starts at 50
and is changed in code, not through runtime configuration.

Simulation event categories may eventually be filtered, possibly through user
configuration.

## Event timestamps

Each plugin event has a game-world timestamp containing year, month, day, and
tick. The plugin obtains these values from DFHack's World module. Year plus tick
is sufficient for complete simulation-time ordering; month and day are included
for readability.

## State and persistence principles

Dwarf Fortress should remain authoritative for current game state. The companion
should not duplicate current state merely for convenience if it can be queried
reliably through DFHack.

Some companion-owned information may nevertheless require persistence,
including:
- significant observations whose historical occurrence cannot be reconstructed
  later;
- player intentions and priorities;
- chat history and useful conversational/derived context;
- other site-specific context needed to resume work with the fortress.

Named AI configurations, including their system text, method, URL, and model ID,
are machine-specific and remain in per-user local application data. API keys
remain in the operating system credential vault and are scoped to stable
configuration IDs. Neither AI configuration nor credentials are included in
Dwarf Fortress save data; each machine must be configured separately.

Site-related chat state is stored with Dwarf Fortress game saves by coordination
between the companion process and the Lua scripts. During site load, the plugin
retrieves that item and sends the snapshot to the Companion to restore.
Snapshots are versioned and include simulation time at save, allowing the AI to
recognize that play may have occurred since the saved context was captured (for
example, while the companion was not running). For now, assume the snapshot fits
in a single gRPC call; define and validate practical size limits before relying
on this in production.

Site-related data may include site-specific settings, e.g. what events to ignore
or pass on to the companion. Snapshot contents should be limited to
Companion-owned, site-specific context and should not duplicate game state that
can be queried from DFHack. Since game saves may be synchronized or shared,
persisted chat and derived context should be treated as user data and kept
within a validated size limit.

Saving and restoring site-related data is under control of the DFHack plugin.
There is no opportunity to save state separately during shutdown of DFCompanion.

## Cost

The project is a hobby project and should keep recurring costs low. Continuous
observation should happen locally without requiring continuous AI API calls.
Model/provider choice and detailed cost controls remain later decisions.

The AI integration uses a provider-neutral `genai` chat layer with OpenAI Chat
Completions, OpenAI Responses, and Anthropic methods; model IDs and HTTPS
service URLs are configurable per named profile. User-editable system text and
per-configuration vault-backed API keys are supported. The separate
`evansenter/genai-rs` project targets Google's Gemini API and is not suitable
for these selected methods; Gemini remains out of scope. In the current
implementation, API calls happen for player chat and retries, optionally within
5 seconds after a game event, and optionaly every 30 seconds while the game is
running (automatic turns). The AI may abstain from producing a user-visible
response when an automatic turn does not merit one. The worker estimates a
16,000-token input budget, compacting oldest input history, and caps generated
output at 8,192 tokens. On API failure, pending chat and reports remain
available for retry and the UI shows the error.

## Learning goals

The project should expose, rather than hide, important AI-agent concepts: tool use, context construction, memory, event handling, persistence, and the boundary between deterministic local processing and LLM reasoning.

Initially, prefer simple mechanisms and let the AI infer higher-level meaning from factual observations. Add local semantic inference only where experience demonstrates its value.

## Implementation preferences

- Rust is the implementation language for the main application.
- Use CMake as the top-level build orchestrator, with Cargo managing Rust
  dependencies and compiling the Rust code.
- The HTTP service is local-only and serves a browser UI, with SSE for
  server-to-browser updates.
- The observation/query interface should ideally become useful independently of
  the particular AI provider or chat UI.

## Security
- Bind the HTTP service to loopback only.
- Treat browser access to localhost as a security boundary: validate request
  origin/host, protect state-changing endpoints against cross-site requests,
  avoid permissive CORS, and do not put credentials in URLs or logs.
- Store AI credentials in the operating system credential vault; never in the
  app settings file, a game save, logs, or URLs.
- Local  traffic is unencrypted and unauthenticated. A shared-secret mechanism
  may be added later to protect against forged local messages.

## Initial development setup
- Install Rust through `rustup` (including Cargo) and install CMake.
- Install the platform's linker/build tools required by the selected Rust
  target.
- Use the `dfhack-remote` crate for DFHack remote state queries.
- Keep Cargo/CMake build outputs out of source control while committing
  `Cargo.lock` for reproducible application builds.

## Explicitly out of scope
- Playing Dwarf Fortress autonomously.
- Choosing fortress geometry or issuing game commands for the player.
- Screenshot-based state inference when structured DFHack access is available.
- Elaborate agent frameworks before their value has been demonstrated.
- Game modes other than fortress mode at an active site.
