# Architecture of DFCompanion

Current architecture and principles to adhere to.

## Vision

Build an open-source conversational companion for Dwarf Fortress that can
observe a live fortress through DFHack and give an AI enough context to answer
player questions intelligently and collaborate with the user on gameplay
strategy.

The goal is assistance and understanding, not autonomous play. The player
remains responsible for interacting with and controlling Dwarf Fortress. The
only effect the companion can have on the game is to pause it.

The companion will not expose information to the AI that is not accessible to
the user, although it may receive information organized in ways that differ from
the UI for the convenience of the AI.

## Explicitly out of scope

- Playing Dwarf Fortress autonomously.
- Choosing fortress geometry or issuing game commands for the player.
- Screenshot-based state inference when structured DFHack access is available.
- Elaborate agent frameworks before their value has been demonstrated.
- Game modes other than fortress mode at an active site.

## Primary actors and boundaries

- **Player:** interacts directly with Dwarf Fortress and converses with the AI
  through the companion.
- **AI:** converses with the player and receives/queries game observations
  through the companion.
- **Game environment:** Dwarf Fortress is the authoritative simulation; DFHack
  is the observation interface into it.
- **Companion system:** mediates AI/game interaction and may retain information
  that cannot reliably be reconstructed from current game state.

Direct player/game interaction is outside the companion's scope.

## Major components

DFCompanion runs as a single asynchronous process with an embedded HTTP service,
a companion worker, and a lifecycle poller. The service binds to
`127.0.0.1:3000`, serves the HTMX-based UI, accepts user input, and provides
separate endpoints for the DFHack `dfcompanion` plugin's reports and lifecycle
updates.

`CompanionApp` owns the shared application state and bounded event queue. User
messages and game reports arrive as `ExternalEvent` values on the queue (capacity
50). The worker feeds them to `ChatSession`; lifecycle pushes update the shared
loading state directly. The DFHack plugin pauses the game and alerts the player
if a report cannot be delivered, rather than silently losing it.

`ChatSession` is the conversation state machine. It contains pending inputs,
conversation history, the in-flight turn or compaction, automatic-turn timers,
and rollback checkpoints. It performs no I/O: it consumes `ChatEvents` and
returns ordered `Effect` values. `CompanionApp` performs those effects, including
publishing UI state, starting asynchronous AI work, pausing a blocked game, and
updating the notification history. Slow operations therefore do not hold the
worker loop or Dwarf Fortress.

`AiRuntime` is stateless with respect to the conversation. It executes one AI
turn or one history-compaction request from the supplied configuration, history,
and prompt. Game tools are application-owned rather than provider-owned and may
use both DFHack RemoteFortressReader queries and the companion's `rpc.dfcompanion`
Lua module. The latter is distinct from the event-forwarding
`dfcompanion` plugin.

The current lifecycle state is maintained by pushes from the DFHack plugin and
by a 20-second polling fallback. Its states distinguish DFHack unreachable, no
world, world loaded, and fortress map loaded. Fortress-data tools are gated
until a fortress is loaded; ordinary conversation remains available at every
loading state.

## Semantic inference

Local code should avoid interpreting strategic implications. It should report
sufficiently rich facts and allow the AI to reason about them.

For example, the adapter might report a unit death plus directly retrievable
information about the unit, rather than declaring the death strategically
important.

Higher-order locally synthesized events remain an option for situations where
they are needed to trigger asynchronous wakeups, reduce AI usage, or capture
information unavailable from native events.

## Localhost security

- Bind the HTTP service to loopback only.
- Treat browser access to localhost as a security boundary: validate request
  origin/host, protect state-changing endpoints against cross-site requests,
  avoid permissive CORS, and do not put credentials in URLs or logs.
- The service validates the exact local host. Browser state changes require the
  expected `Origin`; plugin endpoints reject browser-originated requests
  instead.
- Store AI credentials in the operating system credential vault; never in the
  app settings file, a game save, logs, or URLs.
- Local traffic is unencrypted and unauthenticated. A shared-secret mechanism
  may be added later to protect against forged local messages.

## AI context and provider

Provider portability is implemented with the provider-neutral `genai` Rust
library, using its adapters. The library also handles normalized tool-call exchange;
application-owned tool implementations live separately from request
orchestration.

Google Gemini is intentionally unsupported because the limitations on age and
application purpose in its [terms](https://aistudio.google.com/docs/terms) do
not permit the project's intended use.

Currently, player messages trigger one sequential AI turn and reports accumulate
as context; optionally, the AI also gets an automatic turn within 5 seconds of a
game event and every 30 seconds while the game runs, and may abstain by replying
`NO_COMMENT`. Automatic event turns and periodic turns are independently
enabled and are off by default.

Prompts have an estimated 16,000-token input budget and a hard 8,192-token output
cap. Prompt construction accounts for the configured system text, tool
definitions, history, and pending content, then includes a fitting contiguous
prefix of user requests or reports; content that does not fit remains queued.
Large completed histories are compacted before another turn by replacing older
messages with an AI-written rolling summary while retaining the six most recent
messages verbatim. The summary is carried into subsequent requests and is
treated as untrusted conversation data, not as instructions.

Within a turn, `AiRuntime` repeatedly sends the growing `ChatRequest` until the
provider returns a final text response. Each tool-use round appends the
provider's assistant message and one combined message containing every result
from that batch. The combined result message is required for Anthropic-style
tool-use APIs. Tool messages remain only in the current request; they do not
become conversation-history messages after the turn.

The 16,000-token estimate covers the initial prompt, not the tool messages
accumulated during subsequent request rounds. Tool calls are therefore bounded
in number by an application budget, and tools should return compact data rather
than relying on the initial prompt limit to constrain their replies.

Turns run sequentially. A failed turn or compaction blocks further AI
processing, pauses the game when possible, and retains pending inputs plus the
pre-failure history for the player to resume or roll back. A rollback restores
the corresponding pre-turn history and notification state, cancels in-flight
work, and returns dropped player text to the editor.

## Game timestamps

The game date is written `YYYY-MM-DD[:T]` (year, month 1-12, day 1-28, and T =
ticks into the day, 0-1199), the same string used by the tools and by forwarded
events. Because ticks are relative to the day, the string sorts chronologically
when its fields are compared numerically (year, month, day, then tick). Events
carry no real-world timestamp. The Lua module derives the date from
`dfhack.world.ReadCurrentYear`, `ReadCurrentTick` (ticks since the start of the
year, so month = tick / 33600 + 1, day = (tick % 33600) / 1200 + 1, and ticks
into the day = tick % 1200).

## Text encoding

DFHack's Lua API sometimes hands back strings in Dwarf Fortress's CP437
encoding, and the documentation does not always say so. The Lua module therefore
passes every string taken from the game (report text, unit and squad names,
professions, job names, position titles, item descriptions) through
`dfhack.df2utf` before returning it, so all JSON sent to the service is UTF-8.
Any new text read from the game must do the same. Strings that come from Lua
tables of ASCII identifiers, such as enum names, need no conversion.

## Data persistence

Dwarf Fortress should remain authoritative for current game state. The companion
should not duplicate current state merely for convenience if it can be queried
reliably through DFHack.

AI configuration metadata and the active selection are stored in a local
settings file. API keys are stored separately in the operating system credential
vault. Conversation history and notification state are currently in memory for
the lifetime of the companion process.

Persistence of chat history as part of the Dwarf Fortress save is a planned
feature, not a current capability. The persisted form should include enough
context to continue a companion chat session:

- Must include game timestamp so the AI can estimate how stale it is compared to
  the running game state.
- Must not include sensitive or machine-specific data.
- Should also avoid bloat from ephemeral game data that can be reconstructed
  from AI tools.
- Will be sent to the Lua component after every completed user AI interaction so
  that it will be persisted when the user saves a game.
- Will be loaded the first time that DFCompanion interacts with currently loaded
  fortress, before any AI interaction in that fortress's context.

## Learning goals

The project should expose, rather than hide, important AI-agent concepts: tool
use, context construction, memory, event handling, persistence, and the boundary
between deterministic local processing and LLM reasoning.

Initially, prefer simple mechanisms and let the AI infer higher-level meaning
from factual observations. Add local semantic inference only where experience
demonstrates its value.
