---
applyTo: "src/web.rs,src/web/**"
---

# Presentation

- Keep the main Web page compact and focused on interaction with the companion AI.

# Design

- Use idiomatic HTMX constructs.
- Use custom javascript sparingly for behaviors that can't be implemented in a straightforward fashion with HTMX.
- Keep complex logic that affects application behavior out of POST handlers.
  - Exceptions are allowed for inputs that affect only presentation.
  - Prefer sending an appropriate QueueItem type to the worker thread.
