# Dwarf Fortress AI Companion: Project Requirements Summary

## Vision
Build an open-source conversational companion for Dwarf Fortress that can observe a live fortress through DFHack and give an AI enough context to answer player questions intelligently.

The goal is assistance and understanding, not autonomous play. The player remains responsible for interacting with and controlling Dwarf Fortress.

## Primary actors and boundaries
- **Player:** interacts directly with Dwarf Fortress and converses with the AI through the companion.
- **AI:** converses with the player and receives/querys game observations through the companion.
- **Game environment:** Dwarf Fortress is the authoritative simulation; DFHack is the observation interface into it.
- **Companion system:** mediates AI/game interaction and may retain information that cannot reliably be reconstructed from current game state.

Direct player/game interaction is outside the companion's scope.

## MVP objectives
1. Receive selected fortress simulation events from a separate DFHack plugin over a gRPC/protobuf interface.
2. Put plugin events and player chat into one Companion input queue, distinguished by category.
3. Maintain a conversational UI in which the AI can combine player questions with live game context and query current game state through bounded, read-only tools.
4. Process events asynchronously, batch them on a short idle timeout or queue-pressure threshold, and offer each batch to the AI without requiring a user-visible response for every batch.
5. Ask the plugin to pause the game if the bounded event queue fills.
6. Persist Companion-owned, site-specific context alongside the corresponding game site so it can be restored on another machine.
7. Keep installation and deployment practical on both the gaming desktop and laptop.

The initial scope is fortress mode at an active site; other game modes are not supported.

## Service components
- **Plugin RPC interface:** a gRPC service hosted in the DFCompanion process. The DFHack plugin is its client and submits simulation events and exchanges site-state snapshots during site save/load callbacks.
- **Shared input queue:** a bounded in-memory queue of capacity 50 (a code constant, not initially a user setting). Plugin events carry a category; chat messages use the reserved `user_input` category.
- **Companion worker:** consumes queued events, may batch them, applies AI API rate limits, and presents resulting messages in the shared conversation stream. The user may also prompt the AI directly.
- **HTTP/UI service:** serves a browser interface with a chat prompt, one notification/conversation stream for AI output, and configuration controls. Server-Sent Events (SSE) deliver updates to the page.
- **DFHack access:** the AI may request current game state only through bounded, read-only Companion tools backed by an application-allowlisted set of DFHack remote API calls or plugin-provided procedures. The AI never receives direct access to the remote interface. Initial query areas are a fortress-health overview (including status-header indicators such as happiness-level and food/drink counts), personnel and individual details, defined locations/zones, and stock and stockpile inventories. Check game/DFHack version compatibility before any tool query; warn the user or request confirmation if versions differ.

The Companion worker supports OpenAI Chat Completions, OpenAI Responses, and Anthropic through the provider-neutral `genai` Rust library. Users can create named configurations with an interaction method, URL, model ID, API key, and editable system text, then select one for chat. A static built-in provider catalog offers editable setup presets; the provider choice is not persisted in a configuration. Configuration metadata is stored per-user locally and each API key is stored separately in the OS credential vault. New installations start without configurations; the previous single-provider settings are not migrated. Google Gemini is intentionally unsupported because its terms do not permit the project's intended use. Player chat is submitted to the same queue as simulation events. Reports are accumulated as context and do not independently trigger model calls; one sequential AI turn is made per player message.

## Event policy
Simulation event categories may eventually be user-configurable through the UI:

- **Wake:** retain the event and invoke the AI promptly.
- **Defer:** retain it and provide it the next time the AI is invoked, whether by another wake event or by player chat.
- **Ignore:** do not retain it for AI consumption.

The `user_input` category always enters the Companion's processing path. Numeric event priority is not part of the initial event contract; if needed later, priority can be configured per category. The first implementation processes accepted events without category filtering.

When the queue is full, the RPC response tells the plugin that a pause is requested and that the incoming event was not accepted. The plugin is responsible for pausing the game. DFCompanion also publishes a queue-full notification to the UI. Queue capacity starts at 50 and is changed in code, not through runtime configuration.

## Event timestamps
Each plugin event has a UTC real-world timestamp with sub-second precision, represented as a protobuf `google.protobuf.Timestamp`, and a game-world timestamp containing year, month, day, and tick. The plugin obtains these values from the system clock and DFHack's World module. Year plus tick is sufficient for complete simulation-time ordering; month and day are included for readability.

## State and persistence principles
Dwarf Fortress should remain authoritative for current game state. The companion should not duplicate current state merely for convenience if it can be queried reliably through DFHack.

Some companion-owned information may nevertheless require persistence, including:
- significant observations whose historical occurrence cannot be reconstructed later;
- player intentions and priorities;
- chat history and useful conversational/derived context;
- other site-specific context needed to resume work with the fortress.

Named AI configurations, including their system text, method, URL, and model ID, are machine-specific and remain in per-user local application data. API keys remain in the operating system credential vault and are scoped to stable configuration IDs. Neither AI configuration nor credentials are included in Dwarf Fortress save data; each machine must be configured separately.

Site-related state is stored through DFHack's Persistence module as named items, using its supported `int`, `bool`, and `std::string` types; this associates it with a fortress and incorporates it into saved data. During site save, the plugin requests a snapshot from the Companion over gRPC and stores the serialized snapshot in a named string item. During site load, the plugin retrieves that item and sends the snapshot to the Companion to restore. Snapshots are versioned and include simulation time at save, allowing the AI to recognize that play may have occurred since the saved context was captured (for example, while the plugin was disabled). For now, assume the snapshot fits in a single gRPC call; define and validate practical size limits before relying on this in production.

Site-related data may include site-specific settings, e.g. what events to ignore or pass on to the companion. Snapshot contents should be limited to Companion-owned, site-specific context and should not duplicate game state that can be queried from DFHack. Since game saves may be synchronized or shared, persisted chat and derived context should be treated as user data and kept within a validated size limit.

Saving and restoring site-related data is under control of the DFHack plugin. There is no opportunity to save state separately during shutdown of DFCompanion.

## Cost
The project is a hobby project and should keep recurring costs low. Continuous observation should happen locally without requiring continuous AI API calls. Model/provider choice and detailed cost controls remain later decisions.

The AI integration uses a provider-neutral `genai` chat layer with OpenAI Chat Completions, OpenAI Responses, and Anthropic methods; model IDs and HTTPS service URLs are configurable per named profile. User-editable system text and per-configuration vault-backed API keys are supported. The separate `evansenter/genai-rs` project targets Google's Gemini API and is not suitable for these selected methods; Gemini remains out of scope. In the current implementation, API calls happen only for player chat or retries. The planned event flow will also offer event batches to the AI for consideration; the AI must be allowed to abstain from producing a user-visible response when a batch does not merit one. The worker estimates a 16,000-token input budget, dropping oldest pending reports first and then older conversation history, and caps generated output at 4,096 tokens. On API failure, pending chat and reports remain available for retry and the UI shows the error. Conversation state is currently in memory only.

## Near-term implementation priorities
1. Add read-only AI tools for inspecting fortress health, current personnel (including useful details about an individual), defined locations/zones, and stock and stockpile inventories. The overview should include useful status-header indicators such as happiness-level counts and food/drink counts, once their DFHack source and semantics are confirmed. Keep replies curated, aggregated where appropriate, filtered/paginated for large rosters, and bounded by result-size and token budgets; do not pass through arbitrary remote calls or expose DFHack remote access to the model.
   - Preflight the running game and DFHack versions before tools are callable. On mismatch, tell the user and decide whether explicit confirmation is needed before continuing.
2. Batch simulation events for AI consideration after a configurable-in-code idle interval of a few seconds, or when the shared queue exceeds half its capacity. Each batch gives the AI an opportunity to respond, but it is not required to produce a visible response for every batch.
3. Persist site-specific Companion state with the game save and restore it on site load. Keep machine-specific AI configuration and API credentials out of the game save: provider/model/endpoint settings stay in per-user local storage and credentials stay in the operating system credential vault.

## Learning goals
The project should expose, rather than hide, important AI-agent concepts: tool use, context construction, memory, event handling, persistence, and the boundary between deterministic local processing and LLM reasoning.

Initially, prefer simple mechanisms and let the AI infer higher-level meaning from factual observations. Add local semantic inference only where experience demonstrates its value.

## Implementation preferences
- Rust is the implementation language for the main application.
- Use CMake as the top-level build orchestrator, with Cargo managing Rust dependencies and compiling the Rust code.
- Use gRPC over HTTP/2 for the plugin-to-Companion interface, with project-owned protobuf service/message definitions generated by `protoc`. The service runs in the same process as the Companion worker; the DFHack plugin is the gRPC client. Initial fixed ports are HTTP `127.0.0.1:3000` and gRPC `127.0.0.1:50051`.
- Exchange versioned, simulation-time-stamped site-state snapshots through gRPC; the plugin stores the serialized snapshot in a named `std::string` item using DFHack's Persistence module during site save/load callbacks. The callback stream is not used for persistent state. Assume a snapshot fits in one gRPC call provisionally.
- A separate C++ DFHack plugin is part of the planned architecture.
- The HTTP service is local-only and serves a browser UI, with SSE for server-to-browser updates.
- The observation/query interface should ideally become useful independently of the particular AI provider or chat UI.

## Security
- Bind the HTTP service to loopback only.
- Treat browser access to localhost as a security boundary: validate request origin/host, protect state-changing endpoints against cross-site requests, avoid permissive CORS, and do not put credentials in URLs or logs.
- Store AI credentials in the operating system credential vault; never in the app settings file, a game save, logs, or URLs.
- Initial local gRPC traffic is unencrypted and unauthenticated. A shared-secret mechanism may be added later to protect against forged local messages.

## Initial development setup
- Install Rust through `rustup` (including Cargo) and install CMake.
- Install the platform's linker/build tools required by the selected Rust target.
- Let Cargo provision the host `protoc` through the pinned-by-lockfile `protoc-bin-vendored` build dependency for Rust protobuf generation. The C++ plugin build will need its own code-generation integration.
- Select and configure compatible Rust and C++ gRPC/protobuf code-generation toolchains.
- Set up the C++ toolchain and DFHack SDK needed to build and test the planned plugin.
- Use the `dfhack-remote` crate for DFHack remote state queries if its dependency graph can be resolved cleanly; it is separate from the plugin-to-Companion RPC interface.
- Keep Cargo/CMake build outputs out of source control while committing `Cargo.lock` for reproducible application builds.

## Explicitly outside the initial scope
- Playing Dwarf Fortress autonomously.
- Choosing fortress geometry or issuing game commands for the player.
- Screenshot-based state inference when structured DFHack access is available.
- Elaborate agent frameworks before their value has been demonstrated.
- Game modes other than fortress mode at an active site.
