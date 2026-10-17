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
The main application is written in Rust, so S-expressions are not the assumed default for normalized observations or LLM context. The planned DFHack remote API integration will use the `dfhack-remote` crate, which supplies its own protobuf message code generation, once its transitive dependencies can be resolved cleanly. A project-owned `protoc` is deferred unless custom schemas or a native DFHack plugin make it necessary. Keep the normalized representation easy to inspect, log, persist, and process.

Illustrative shape only:

    (event
      :type unit-death
      :game-time (...)
      :unit (:id 847 :name "Oddom" :profession "Axedwarf")
      :location (:x 143 :y 87 :z 45)
      :details (...))

A component boundary may use another wire format if tooling requires it; the wire format need not dictate the application's internal or AI-facing representation.

## Game timestamps
Native events may not contain a complete timestamp. A timestamp can potentially be assembled when an event is received using DFHack world-state values such as current year, month, day, and tick information.

The normalized observation should include a canonical machine-usable game-time value sufficient for ordering and elapsed-game-time calculations. Human calendar fields can accompany it where useful.

## Wake, defer, ignore
A deferred event should normally become context the next time the AI runs, regardless of what causes that invocation. Thus a later wake event should bring pending deferred observations along with it, just as a new player chat message would.

Questions left open include retention/consumption rules after deferred observations have been presented.

## Native events first
The MVP should initially use native DFHack events only. The first technical research task is to inventory EventManager/eventful behavior:

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

## Current architectural hypothesis
A plausible arrangement is:

    Browser UI
        |
    local companion service <----> AI API
        |
    DFHack adapter
        |
    Dwarf Fortress

The companion would run locally beside the game. The browser UI would provide chat, configuration, and eventually observation/event-policy inspection. HTTP and a browser UI are conveniences, not fundamental requirements of the core observation system.

## Portability and persistence
The same fortress is intended to be played on a gaming desktop and personal laptop, with DF state synchronized through OneDrive.

If companion-owned history becomes necessary, it will also need a safe portability/synchronization strategy. No database or storage technology has yet been selected.

## Early proof-of-concept milestone
Receive a useful native DFHack event, enrich it with enough game state to make the event intelligible, convert it into the normalized representation, and demonstrate that an LLM can correctly understand what happened.
