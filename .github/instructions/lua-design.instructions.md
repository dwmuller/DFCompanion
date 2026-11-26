---
applyTo: "lua/**"
---

- Keep the modules in `lua/dfcompanion/internal` each focused on one game or
  tool concept, and keep dependencies acyclic as game tools grow. Consult with
  the user before creating a new concept module; prefer using an existing module
  over expanding the facade. If the user cannot be consulted in this session,
  do not create a new concept module. Instead, place the handler in the closest
  existing module and add a code comment starting with `TODO(module-proposal):`
  describing the proposed module name and its purpose.
