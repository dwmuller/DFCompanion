## Current Focus

What feature or bug we're actively working on.

- TBD

## Next Steps

Immediate next 1 to 3 tasks to work on.

- Decide how Companion-owned chat history should travel with the game save:
  follow the existing Lua + HTTP pattern used for game events (Companion
  exposes a history-snapshot GET/POST endpoint; the Lua script polls it and
  stores the result via `dfhack.persistent.saveSiteDataString`, restoring it
  on load), or build the originally-sketched C++ DFHack plugin with gRPC
  save/load callbacks. The current `df-ai-design-notes.md` and
  `df-ai-project-requirements.md` describe the latter, but no C++ plugin
  exists yet; only the Lua script and HTTP API do.
- Once decided, implement history persistence: a serializable snapshot of
  `History` (the rolling summary plus recent messages already produced by
  compaction), new Companion endpoints or RPCs to fetch/restore it, and the
  corresponding Lua-side read/write of DFHack's per-site persistent storage.
- Update the design/requirements docs' persistence sections to match
  whichever mechanism is implemented.

## Known Issues / Blockers

- None.