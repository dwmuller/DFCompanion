# Dwarf Fortress AI Companion: Early Design Notes

These notes capture lower-level ideas discussed so far. They are exploratory, not committed architecture.

## Observation pipeline
A useful conceptual pipeline is:

    Dwarf Fortress / DFHack
              |
              v
        raw native event
              |
              v
      normalized observation
              |
       +------+------+
       |             |
    event policy   retained context
       |
       v
   AI invocation <---- player chat

Raw DFHack representations should be normalized before presentation to an AI. The normalized form should contain facts rather than strategic interpretation.

## Structured representation
The plugin-to-Companion interface will use gRPC over HTTP/2 with project-owned protobuf messages and `protoc`-generated code. The Rust build provisions a host compiler through Cargo's `protoc-bin-vendored` dependency. The gRPC server runs in the same process as the Companion worker; the DFHack plugin is its client. The Companion may separately use `dfhack-remote` for queries against DFHack's remote API; that is distinct from the custom plugin-to-Companion interface. Keep normalized observations easy to inspect, log, persist, and process.

Each simulation event contains a category, a UTC real-world timestamp, a game-world timestamp, and descriptive text. The real timestamp uses protobuf `google.protobuf.Timestamp` for sub-second precision. Game time is represented by year, month, day, and tick from DFHack's World module; year plus tick is sufficient for complete ordering. Numeric priority is omitted initially; if needed later, priority can be associated with categories.

    SimulationEvent {
      category,
      real_world_time_utc,
      game_world_time: { year, month, day, tick },
      description
    }

The gRPC service needs a procedure to submit a simulation event and procedures to exchange site-state snapshots with the plugin. The `std::ostream`/`std::istream` supplied to DFHack's `plugin_save_site_data` and `plugin_load_site_data` callbacks is not the persistence store. The plugin must use DFHack's Persistence module, which stores named items with `int`, `bool`, or `std::string` values. During site save, request a snapshot from the Companion over gRPC and serialize it into a named string item; during site load, retrieve and deserialize that named item, then send it to the Companion for restoration. These calls are provisionally assumed to fit in a single gRPC request/response. The Persistence item is associated with the saved site; include world/site identifiers if available, but a new unsaved game may not have a save identity.

Each snapshot must include a schema/version identifier and the simulation time at which it was written. This timestamp lets the AI notice a gap between saved context and current game time, such as a play session that occurred while the plugin was disabled. The serialized snapshot must fit in the Persistence module's string representation and in a single gRPC call; exact payload contents, string encoding/serialization format, and safe maximum size still need definition and testing.

Pausing the game under event-queue pressure remains a requirement, but its call direction and mechanism are not part of the confirmed plugin-to-Companion procedures yet. Determine whether to use DFHack's existing remote API or add an explicit plugin command/callback.

## Lua transport feasibility prototype
As a low-cost alternative to immediately building a native plugin, test whether a DFHack Lua script can submit events to the existing HTTP service. The installed DFHack 53.16-r2 Lua environment includes a JSON module and a TCP socket wrapper, so the prototype sends JSON over HTTP/1.1 to `127.0.0.1:3000/api/plugin/events` without protobuf code generation. The service generates a fresh 256-bit bearer token at startup and displays a copyable DFHack command containing it on the local web page; the script receives the token as an argument. It supports DFHack's standard enable/disable lifecycle and keeps its enabled state only in memory. When enabled, it subscribes to eventful's `REPORT` event and forwards report text converted from CP437 to UTF-8, using separate categories for announcements and other reports. Since DFHack warns this event is more frequent than announcements, event volume and synchronous HTTP latency need in-game evaluation. This is an experiment, not yet a decision to replace the planned gRPC interface.

## Game timestamps
The plugin supplies each event's real-world timestamp in UTC using protobuf `google.protobuf.Timestamp`, which includes seconds and nanoseconds. It obtains simulation time from DFHack's World module as year, month, day, and tick. Year plus tick is the canonical game-time ordering; month and day are retained as readable calendar fields.

## Wake, defer, ignore
A deferred event should normally become context the next time the AI runs, regardless of what causes that invocation. Thus a later wake event should bring pending deferred observations along with it, just as a new player chat message would.

Player chat uses the reserved `user_input` category and enters the same queue as simulation events. The Companion worker consumes both from one source. Event categories may later be mapped to Wake/Defer/Ignore; the first implementation processes all accepted categories. Queue capacity is a code constant set to 50.

Currently, events are accumulated locally and included with the next player-triggered AI request. Planned behavior is to collect events into a batch and offer the batch to the AI after a configurable-in-code idle interval of a few seconds, or sooner when the shared input queue exceeds half its capacity. This gives the AI a chance to surface important developments without treating every batch as requiring a user-visible response. Define an explicit abstention/no-response convention so routine batches do not create empty or distracting chat messages. Player chat remains an immediate reason to invoke the AI and should include the accumulated context. If the queue is full, the gRPC response indicates that the plugin should pause the game, and DFCompanion publishes a queue-full notification to the UI. The incoming event is rejected rather than silently dropped. Numeric priority-based dropping and de-duplication are deferred.

## Plugin event selection
The DFHack plugin selects which game events to send. Research the EventManager/eventful surface to inform plugin selection:

- available event types;
- payloads supplied by each event;
- Lua accessibility;
- event frequency/granularity;
- additional state lookups needed to make events intelligible;
- important simulation changes that native events do not expose.

## Periodic state monitoring: later possibility
Some important conditions may not have useful native events. Examples include trends or threshold crossings in drink stocks.

If needed later, configurable monitors could sample selected game state and feed observations or synthesized events into the same stream as native events.

Polling should be scheduled relative to **game time**, not merely wall-clock time, since the simulation may be paused or run at different rates.

This is deliberately deferred until native event coverage has been evaluated.

## Semantic inference
Initially, local code should avoid interpreting strategic implications. It should report sufficiently rich facts and allow the AI to reason about them.

For example, the adapter might report a unit death plus directly retrievable information about the unit, rather than declaring the death strategically important.

Higher-order locally synthesized events remain an option for situations where they are needed to trigger asynchronous wakeups, reduce AI usage, or capture information unavailable from native events.

## Service architecture
The planned arrangement is:

    Browser UI <-- HTTP + SSE --> Companion service <-- gRPC / protobuf --> DFHack plugin
                                       |                           |
                                       |                           v
                                       |                    Dwarf Fortress
                                       |
                                       +---- AI provider API
                                       |
                                       +---- DFHack remote API

The service has three logical parts in one Rust process: a gRPC server accepting plugin events and site save/load notifications, a queue-consuming Companion worker that interacts with the AI, and an HTTP service for the UI. The plugin initiates gRPC calls to the Companion. AI messages from event processing and player chat appear in one conversation/notification stream.

The service runs locally beside the game. The UI includes a chat prompt, a shared output stream, and configuration controls for Anthropic and OpenAI-compatible APIs, including presets for OpenAI and Groq. Model IDs are free-form. Provider/model/custom-endpoint settings are stored in per-user local application data; API keys are stored in the operating system credential vault, with custom endpoint keys scoped to their base URL. The UI warns users to review the selected provider's privacy and data-use terms and links the policy when known. Custom endpoint policy links cannot be verified automatically.

The initial fixed endpoints are HTTP `127.0.0.1:3000` and gRPC `127.0.0.1:50051`. Local gRPC calls are initially unencrypted and unauthenticated; a shared-secret mechanism may be added later. HTTP chat submissions validate the expected Host and Origin.

## Portability and persistence
The same fortress is intended to be played on a gaming desktop and personal laptop, with DF state synchronized through OneDrive.

Settings persist independently from site state. During `plugin_save_site_data`, the plugin requests a snapshot from the Companion and writes its serialized form as a named `std::string` through DFHack's Persistence module; during `plugin_load_site_data`, it retrieves the named value and sends it to the Companion for restoration. The callback streams are for other callback data, not this persistent state. Persistence associates the item with the saved site, avoiding the need for the Companion to identify multiple named saves itself. Site save/load callbacks also provide the points to flush pending event batches into the snapshot or restore them from it. Snapshots should contain the site-specific AI context needed to resume, pending events, a format version, and simulation timestamp; decide whether full chat history is necessary, and bound its contribution if included. Do not include provider/model/endpoint settings or API keys in the game save: those are machine-specific and remain in per-user settings and the operating system credential vault. Persisted chat and derived context are user data and may be synchronized with game saves, so communicate that clearly and keep the snapshot compact. Assume a snapshot fits in one gRPC call and one Persistence string for now, but validate its size and behavior before depending on this assumption. The string encoding/serialization format is undecided. Consider concurrent OneDrive synchronization of game saves.

## Localhost security
Loopback binding alone does not prevent a malicious website from attempting requests to a local service. Use strict Origin/Host checks, CSRF defenses for state-changing routes, no permissive CORS, and avoid exposing credentials in URLs, logs, or browser event streams. API keys are entered through the local UI, stored in the operating system credential vault, and sent to the selected provider only in authentication headers.

## AI context and provider
Provider portability is implemented through a provider-neutral chat request, an Anthropic adapter, and one OpenAI-compatible chat-completions adapter used by OpenAI, Groq, and user-configured endpoints. Google Gemini is intentionally unsupported because its terms do not permit the project's intended use. Currently, player messages trigger one sequential AI turn and reports accumulate as context. The planned event-batch path will also offer accumulated reports for AI consideration and must support an explicit no-response outcome. The worker estimates a 16,000-token input limit, dropping oldest pending reports first and then older conversation turns, and caps output at 512 tokens. API failures are surfaced while pending chat and reports are retained for retry. Conversation state is currently in memory only; settings and credentials persist separately.

## Near-term implementation priorities
Implement the following work in this order:

1. **Read-only game-state tools.** Let the AI request bounded queries for the fortress personnel roster and useful details about an individual, defined locations/zones, and stock and stockpile inventories. Prefer on-demand queries over sending large snapshots on every turn. Normalize results into factual, size-bounded responses; investigate DFHack remote API coverage and use plugin-provided procedures only where needed. Keep these tools read-only and make unavailable/stale game state explicit to the AI.
2. **Timed and pressure-based event batches.** Add a code-configurable idle timeout of a few seconds and flush accumulated events after that much time without new events; also flush sooner when the shared input queue is more than half full. Offer each flushed batch to the AI, with instructions and a defined response protocol that permit abstaining when there is nothing useful to tell the player. Preserve immediate player-chat behavior and include pending event context with that turn.
3. **Site-state saving.** Use DFHack's site save/load hooks and Persistence module to store and restore versioned Companion state with the corresponding game data. Include only site-specific context needed to resume; keep AI provider/model/endpoint settings and API keys out of game saves. Provider settings remain local to each machine and credentials remain in the OS vault. Define which conversational context to persist, serialization/version migration, practical payload bounds, and user-facing notice that saved conversational context may travel with synchronized game files before implementing.

The AI should receive introductory orientation explaining that DFCompanion is a Dwarf Fortress player's companion, incoming reports are machine-generated snippets from a simulated game rather than messages addressed to the AI, and observations may be incomplete or ambiguous. Provide sensible default orientation text and eventually let the user replace it with their own text. The storage scope and location for this customization remain undecided; do not implement it until that decision is made.

## Open integration decisions
- Select matching Rust/C++ gRPC runtimes and protobuf code-generation tooling.
- Determine which world/site identity fields are available from DFHack and which simulation-time fields should timestamp snapshots.
- For game-state tools, identify the exact useful personnel, individual, zone/location, and inventory fields available through DFHack; define tool schemas, bounded result sizes, stale-state errors, and provider-neutral tool-call handling.
- Select the initial event idle-timeout constant and specify how queue occupancy triggers a batch flush without starving normal processing. Define a stable abstention/no-response protocol and how failures retain batches for retry.
- Define snapshot contents (including whether full chat history is worth persisting), serialization/string encoding, version migration behavior, and practical payload-size limits for gRPC and Persistence string items. AI provider/model/endpoint settings and API keys are excluded from site snapshots.
- Decide how the Companion requests a game pause through the plugin or DFHack remote API.
- Decide future queue recovery behavior after a pause and whether category-based filtering or eviction is needed under sustained overload.
- Add provider-side integration tests and measure token usage against tokenizer-aware estimates before relying on the current context budget.
- Decide where user-replaced AI orientation text belongs (machine-wide settings versus fortress/site-specific state) before implementing customization.
- Select snapshot storage format and identify exactly which Companion state is global versus site-scoped.
- Consider shared-secret authentication for local gRPC calls before using the plugin RPC in a normal gameplay environment.

## Early proof-of-concept milestone
Verify plugin-event delivery through a protobuf gRPC call into the bounded queue, verify that web chat enters the same queue under `user_input`, and test provider requests through the provider-neutral AI interface.
