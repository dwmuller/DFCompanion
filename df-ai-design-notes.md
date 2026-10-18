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
As a low-cost alternative to immediately building a native plugin, test whether a DFHack Lua script can submit events to the existing HTTP service. The installed DFHack 53.16-r2 Lua environment includes a JSON module and a TCP socket wrapper, so the prototype sends a small JSON object over HTTP/1.1 to `127.0.0.1:3000/api/plugin/events` without protobuf code generation. The service generates a fresh 256-bit bearer token at startup and displays a copyable DFHack command containing it on the local web page; the script receives the token as an argument. The service validates the event and token, enqueues it through the same bounded queue as gRPC events, and acknowledges acceptance or queue pressure. The first script is manually invoked and sends a synthetic event; native event subscriptions and enrichment remain to be investigated. This is an experiment, not yet a decision to replace the planned gRPC interface.

## Game timestamps
The plugin supplies each event's real-world timestamp in UTC using protobuf `google.protobuf.Timestamp`, which includes seconds and nanoseconds. It obtains simulation time from DFHack's World module as year, month, day, and tick. Year plus tick is the canonical game-time ordering; month and day are retained as readable calendar fields.

## Wake, defer, ignore
A deferred event should normally become context the next time the AI runs, regardless of what causes that invocation. Thus a later wake event should bring pending deferred observations along with it, just as a new player chat message would.

Player chat uses the reserved `user_input` category and enters the same queue as simulation events. The Companion worker consumes both from one source. Event categories may later be mapped to Wake/Defer/Ignore; the first implementation processes all accepted categories. Queue capacity is a code constant set to 50.

Events may be batched before real AI calls, which must be rate-limited. The first worker uses a mock AI response. If the queue is full, the gRPC response indicates that the plugin should pause the game, and DFCompanion publishes a queue-full notification to the UI. The incoming event is rejected rather than silently dropped. Numeric priority-based dropping and de-duplication are deferred.

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

    HTMX browser UI <-- HTTP + SSE --> Companion service <-- gRPC / protobuf --> DFHack plugin
                                       |                           |
                                       |                           v
                                       |                    Dwarf Fortress
                                       |
                                       +---- AI provider API
                                       |
                                       +---- DFHack remote API

The service has three logical parts in one Rust process: a gRPC server accepting plugin events and site save/load notifications, a queue-consuming Companion worker that interacts with the AI, and an HTTP service for the UI. The plugin initiates gRPC calls to the Companion. AI messages from event processing and player chat appear in one conversation/notification stream.

The service runs locally beside the game. The UI includes a chat prompt, a shared output stream, and configuration controls. Configuration details beyond AI credentials remain open.

The initial fixed endpoints are HTTP `127.0.0.1:3000` and gRPC `127.0.0.1:50051`. Local gRPC calls are initially unencrypted and unauthenticated; a shared-secret mechanism may be added later. HTTP chat submissions validate the expected Host and Origin.

## Portability and persistence
The same fortress is intended to be played on a gaming desktop and personal laptop, with DF state synchronized through OneDrive.

Settings persist independently from site state. During `plugin_save_site_data`, the plugin requests a snapshot from the Companion and writes its serialized form as a named `std::string` through DFHack's Persistence module; during `plugin_load_site_data`, it retrieves the named value and sends it to the Companion for restoration. The callback streams are for other callback data, not this persistent state. Persistence associates the item with the saved site, avoiding the need for the Companion to identify multiple named saves itself. Site save/load callbacks also provide the points to flush pending events into the snapshot or restore them from it. Snapshot contents should include the AI context needed to resume, pending events, a format version, and simulation timestamp; whether to include full chat history remains open. Assume a snapshot fits in one gRPC call and one Persistence string for now, but validate its size and behavior before depending on this assumption. The string encoding/serialization format is undecided. Settings remain independently persisted in user-writable storage suitable for possible Steam distribution. Consider concurrent OneDrive synchronization of game saves.

## Localhost security
Loopback binding alone does not prevent a malicious website from attempting requests to a local service. Use strict Origin/Host checks, CSRF defenses for state-changing routes, no permissive CORS, and avoid exposing credentials in URLs, logs, or browser event streams. AI credentials are entered in the UI and held in memory initially; secure storage across runs is deferred.

## AI context and provider
Provider portability is desirable, but no provider, model, API abstraction, context construction strategy, or rate-limit values have been selected. Begin with factual event batches and player messages, then make context and memory policy explicit as the integration is explored. Provider failures should be surfaced in the UI.

## Open integration decisions
- Select matching Rust/C++ gRPC runtimes and protobuf code-generation tooling.
- Determine which world/site identity fields are available from DFHack and which simulation-time fields should timestamp snapshots.
- Define snapshot contents, serialization/string encoding, version migration behavior, and practical payload-size limits for gRPC and Persistence string items.
- Decide how the Companion requests a game pause through the plugin or DFHack remote API.
- Decide future queue recovery behavior after a pause and whether category-based filtering or eviction is needed under sustained overload.
- Select an AI provider/API and define initial batching, rate limits, context limits, and error/retry behavior.
- Select storage format/location and identify exactly which state is global versus site-scoped.
- Consider shared-secret authentication for local gRPC calls before using the plugin RPC in a normal gameplay environment.

## Early proof-of-concept milestone
Verify plugin-event delivery through a protobuf gRPC call into the bounded queue, verify that web chat enters the same queue under `user_input`, and demonstrate the mock worker response in the SSE notification stream.
