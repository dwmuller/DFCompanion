# Architecture of DFCompanion

Current architecture and principles to adhere to.

## Major components

DFCompanion runs as a single process. The main thread is the companion worker.

An HTTP service embedded in the process serves up the user's UI, and accepts
inputs from both the user and a DFHack plugin named `dfcompanion`. The DFHack
plugin uses this mechanism to send updates about game state and game "reports"
similar to those seen in the game's own UI. Inputs are sent as QueueItem objects
to the companion worker.

CompanionApp maintains state for and implements the code for the companion
worker loop which consumes QueueItem objects. It drives much of the activity
with in the app, especially interactions with the AI.

AI tools are provided which query the live game for data. The queries are
ultimately implemented by a Lua module running under control of DFHack, distinct
from the plugin.

## Semantic inference

Local code should avoid interpreting strategic implications. It should report
sufficiently rich facts and allow the AI to reason about them.

For example, the adapter might report a unit death plus directly retrievable information about the unit, rather than declaring the death strategically important.

Higher-order locally synthesized events remain an option for situations where they are needed to trigger asynchronous wakeups, reduce AI usage, or capture information unavailable from native events.

## Localhost security

Loopback binding alone does not prevent a malicious website from attempting
requests to a local service. Use strict Origin/Host checks, CSRF defenses for
state-changing routes, no permissive CORS, and avoid exposing credentials in
URLs, logs, or browser event streams. API keys are entered through the local UI,
stored in the operating system credential vault, and sent to the selected
provider only in authentication headers.

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

AI configuration and user preferences are stored locally.

Chat history is stored as part of Dwarf Fortress game saves and includes enough
context to continue a companion chat session. These saves must not include
sensitive or machine-specific data. They should also avoid bloat from ephemeral
game data that can be reconstructed from AI tools. This chat history will be
sent to the Lua component after every completed user AI interaction so that it
will be persisted when the user saves a game. It will be loaded the first time
that DFCompanion interacts with currently loaded fortress, before any AI
interaction in that fortress' context.