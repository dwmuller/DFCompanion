# Dwarf Fortress AI Companion

DFCompanion is an early-stage companion for Dwarf Fortress. It connects DFHack observations and player questions in a local web app, with the goal of helping an AI answer questions about a live fortress while leaving control of the game to the player.

This is an experimental prototype, not yet a packaged release. The current build uses mock AI responses; real AI-provider configuration and calls are not implemented.

## Getting started

The current Lua event prototype has been tested with Dwarf Fortress 53.16 and DFHack 53.16-r2 on Windows. You need a local DFHack installation and a DFCompanion build. See [BUILDING.md](./BUILDING.md) for developer setup and instructions to build and install the Lua script.

1. Start DFCompanion. The local web app is at <http://127.0.0.1:3000/>.
2. Copy the `dfcompanion <token>` command shown on the web page.
3. With a fortress loaded in Dwarf Fortress, paste that command into the DFHack console. This enables the script and sends a connection-test event.
4. Ask a question in the web app's Chat box. The current response is a mock, not a real AI answer.

The generated token changes every time DFCompanion starts. Copy the new command from the web page after restarting the service. Do not share the token: it authorizes local Lua event submissions.

## Using the DFHack script

When enabled, `dfcompanion.lua` listens for DFHack `REPORT` events and forwards report text to DFCompanion. Announcements and other reports are labeled separately. DFHack reports occur more often than visible announcements, so the script may send many events. Forwarding is synchronous.

- `disable dfcompanion` stops forwarding reports.
- `enable dfcompanion` re-enables it during the current DFHack session.
- `dfcompanion status` displays its current in-memory state.

The enabled state is not saved; it resets when the map unloads or DFHack exits. If the bounded event queue fills, the script pauses the game and reports the condition. If forwarding fails, the script disables itself and reports the error.

## AI providers and privacy

AI-provider configuration is not available in the current prototype. The first AI integration is planned to support Anthropic, OpenAI, and Google Gemini, with a free-form model ID so users can follow each provider's changing model catalog. Provider/model configuration is intended to be machine-specific, with API keys stored in the operating system's credential vault rather than in a game save.

When AI integration is added, player messages and selected fortress reports will be sent to the chosen provider for inference. The planned setup flow will explain this and require acknowledgement before the first request and after a provider or material privacy-notice change. Provider privacy/data-use policies are not standardized for automatic discovery, so DFCompanion will link to official provider policies rather than copy policy text into the app.

### Model suggestions

Model names, availability, prices, and capabilities change frequently. These are starting points from the providers' public catalogs as checked on **October 18, 2026**, not fixed defaults or endorsements. Check current availability and pricing in your account before use; enter the exact model ID accepted by your provider.

| Provider | Suggested model ID | Why consider it | Current model information |
|---|---|---|---|
| Anthropic | `claude-sonnet-5-5` | A balanced starting point for speed and capability. | [Models overview](https://platform.claude.com/docs/en/models/overview) |
| OpenAI | `gpt-5.4-mini` | A faster, lower-cost option suitable for trying frequent short turns. | [GPT-5.4 Mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini) |
| Google Gemini | `gemini-3.8-flash` | A fast Flash model intended for efficient, high-volume work. | [Gemini models](https://ai.google.dev/gemini-api/docs/models) and [Gemini 3.8 Flash](https://ai.google.dev/gemini-api/docs/models/gemini-3.8-flash) |

Treat these suggestions as perishable: the provider's current catalog and your own results should take precedence.

## Current limitations

- AI responses are mocked; there is no provider setup screen or real AI request yet.
- `REPORT` is the only forwarded gameplay event type so far. The usefulness and coverage of other events have not been evaluated.
- Conversation history and report context are not yet used to construct AI requests.
- Enablement and Companion state are in memory; the Lua script does not persist state to the Dwarf Fortress save.
- This is a development prototype without a user installer or packaged release.

## Developer setup

Build, configure the DFHack install directory, install the Lua script, run tests, and set up VS Code using [BUILDING.md](./BUILDING.md).
