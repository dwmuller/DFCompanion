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
1. Observe meaningful native DFHack events while a fortress is running.
2. Convert raw game information into a stable, AI-friendly structured representation.
3. Maintain conversational interaction in which the AI can combine player questions with live game context.
4. Allow selected events to wake the AI asynchronously, while other events can be deferred or ignored.
5. Avoid continuous use of a metered AI service simply to monitor the game.
6. Keep installation and deployment practical on both the gaming desktop and laptop.

## Event policy
Event categories should be user-configurable through the UI:

- **Wake:** retain the event and invoke the AI promptly.
- **Defer:** retain it and provide it the next time the AI is invoked, whether by another wake event or by player chat.
- **Ignore:** do not retain it for AI consumption.

This is intended to make responsiveness, relevance, and API/token cost experimentally tunable.

## State and persistence principles
Dwarf Fortress should remain authoritative for current game state. The companion should not duplicate current state merely for convenience if it can be queried reliably through DFHack.

Some companion-owned information may nevertheless require persistence, including:
- significant observations whose historical occurrence cannot be reconstructed later;
- player intentions and priorities;
- useful conversational/derived context.

The exact persistence mechanism is intentionally undecided. Any necessary companion state should be portable between the two computers used to play the same synchronized fortress.

## Cost
The project is a hobby project and should keep recurring costs low. Continuous observation should happen locally without requiring continuous AI API calls. Model/provider choice and detailed cost controls remain later decisions.

## Learning goals
The project should expose, rather than hide, important AI-agent concepts: tool use, context construction, memory, event handling, persistence, and the boundary between deterministic local processing and LLM reasoning.

Initially, prefer simple mechanisms and let the AI infer higher-level meaning from factual observations. Add local semantic inference only where experience demonstrates its value.

## Implementation preferences
- Rust is the implementation language for the main application.
- Use CMake as the top-level build orchestrator, with Cargo managing Rust dependencies and compiling the Rust code.
- Plan to use the `dfhack-remote` Rust crate for the initial DFHack remote API integration and its protobuf code generation, once its transitive dependencies can be resolved cleanly; defer a project-owned `protoc` until custom schemas or the likely C++ DFHack plugin require it.
- Defer setting up a C++ toolchain and DFHack plugin component until research shows that a native plugin is needed; use the least-coupled DFHack integration available initially.
- A locally running service with a thin browser UI is the current architectural hypothesis, not yet an immutable requirement.
- The observation/query interface should ideally become useful independently of the particular AI provider or chat UI.

## Initial development setup
- Install Rust through `rustup` (including Cargo) and install CMake.
- Install the platform's linker/build tools required by the selected Rust target.
- Add `dfhack-remote` for DFHack remote API access when its transitive dependencies can be resolved cleanly; no separate `protoc` installation is needed for the crate's generated API.
- Keep Cargo/CMake build outputs out of source control while committing `Cargo.lock` for reproducible application builds.
- Revisit project-owned protobuf code generation and `protoc` when custom schemas or a native plugin are introduced.
- Do not install C++/DFHack SDK tooling for the initial companion unless the chosen DFHack integration requires it.

## Explicitly outside the initial scope
- Playing Dwarf Fortress autonomously.
- Choosing fortress geometry or issuing game commands for the player.
- Screenshot-based state inference when structured DFHack access is available.
- Elaborate agent frameworks before their value has been demonstrated.
- Synthetic higher-order events unless native events prove insufficient for an MVP.
