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
- **AI:** converses with the player and receives/querys game observations
  through the companion.
- **Game environment:** Dwarf Fortress is the authoritative simulation; DFHack
  is the observation interface into it.
- **Companion system:** mediates AI/game interaction and may retain information
  that cannot reliably be reconstructed from current game state.

Direct player/game interaction is outside the companion's scope.

## Major components

DFCompanion runs as a single process. The main thread is the companion worker.

An HTTP service embedded in the process serves up the user's UI, and accepts
inputs from both the user and a DFHack Lua plugin named `dfcompanion`. The
DFHack plugin uses this mechanism to send updates about game state and game
"reports" similar to those seen in the game's own UI. Inputs are sent as
QueueItem objects to the companion worker.

CompanionApp maintains state for and implements the code for the companion
worker loop which consumes QueueItem objects. It drives much of the activity
with in the app, especially interactions with the AI.

AI tools are provided which query the live game for data. The queries are
ultimately implemented by a Lua module running under control of DFHack, distinct
from the plugin.

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
- Store AI credentials in the operating system credential vault; never in the
  app settings file, a game save, logs, or URLs.
- Local  traffic is unencrypted and unauthenticated. A shared-secret mechanism
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
`NO_COMMENT`. The worker estimates a 16,000-token input limit, accounting for
the selected system text, dropping oldest pending reports first and then older
conversation turns, and caps output at 8,192 tokens. API failures are surfaced
while pending chat and reports are retained for retry.

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

AI configuration and user preferences are stored locally.

Chat history is stored as part of Dwarf Fortress game saves and includes enough
context to continue a companion chat session. 
- Must include game timestamp so the AI can estimate how stale it is compared to
  the running game state.
- Must not include sensitive or machine-specific data.
- Should also avoid bloat from ephemeral game data that can be reconstructed
  from AI tools.
- Will be sent to the Lua component after every completed user AI interaction so
  that it will be persisted when the user saves a game.
- Will be loaded the first time that DFCompanion interacts with currently loaded
  fortress, before any AI interaction in that fortress' context.

## Learning goals

The project should expose, rather than hide, important AI-agent concepts: tool
use, context construction, memory, event handling, persistence, and the boundary
between deterministic local processing and LLM reasoning.

Initially, prefer simple mechanisms and let the AI infer higher-level meaning
from factual observations. Add local semantic inference only where experience
demonstrates its value.
