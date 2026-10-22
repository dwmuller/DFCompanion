# Dwarf Fortress AI Companion

DFCompanion is an early-stage companion for Dwarf Fortress. It connects DFHack observations and player questions in a local web app, with the goal of helping an AI answer questions about a live fortress while leaving control of the game to the player.

This is an experimental prototype, not yet a packaged release. It can send chat turns using OpenAI Chat Completions, OpenAI Responses, or Anthropic after you configure a named AI configuration. You can use any provider you want that supports one of these
APIs and for which you have an API key.

## Getting started

The current Lua event prototype has been tested with Dwarf Fortress 53.16 and DFHack 53.16-r2 on Windows. You need a local DFHack installation and a DFCompanion build. See [BUILDING.md](./BUILDING.md) for developer setup and instructions to build and install the Lua script.

1. Start DFCompanion. The local web app is at <http://127.0.0.1:3000/>.
2. Copy the `dfcompanion <token>` command shown on the web page.
3. With a fortress loaded in Dwarf Fortress, paste that command into the DFHack console. This enables the script and sends a connection-test event.
4. Open **Manage AI configurations**, create a configuration, and save its API key (see [Getting AI API access](#getting-ai-api-access)).
5. Select the configuration on the main page. Review the selected service's privacy policy and data-use terms before chatting.
6. Ask a question in the web app's Chat box. Reports received since the previous AI answer are sent along with your question.

DFCompanion and DFHack accept connections from localhost only; there is no shared secret.

## Using the DFHack script

When enabled, `dfcompanion.lua` listens for DFHack `REPORT` events and forwards report text to DFCompanion. Only announcements are forwarded, in the same format as the `get_recent_reports` tool (game date, type, category, text). Forwarding is synchronous. If sending fails, the script pauses the game and raises an alert badge and a dismissible popup; fix or restart DFCompanion and unpause (or run `disable dfcompanion`). The script stays enabled, and the game pauses again if the problem persists.

- `disable dfcompanion` stops forwarding reports.
- `enable dfcompanion` re-enables it during the current DFHack session.
- `dfcompanion status` displays its current in-memory state.

The enabled state is not saved; it resets when the map unloads or DFHack exits. If the bounded event queue fills, the script pauses the game and reports the condition. If forwarding fails, the script disables itself and reports the error.

## AI providers and privacy

Create, rename, select, and delete named AI configurations on the **AI Configurations** page. Each has an interaction method (OpenAI Chat Completions, OpenAI Responses, or Anthropic), API URL, model ID, and editable system text. Built-in provider presets suggest a URL, interaction method, and model choices; these are editable hints, and the provider choice itself is not saved. A **Custom** choice makes no changes to the fields. The main page's dropdown selects which configuration will be used for chat. Use an HTTPS API base URL and the exact model ID accepted by the selected service. HTTP is allowed only for localhost. Configuration metadata is stored in `ai-configurations.json` in per-user local application data. New installations begin with an empty configuration list; existing settings and credential-vault entries are left untouched and are not migrated.

The provider-neutral [`genai` Rust library](https://github.com/jeremychone/rust-genai) supplies the adapters for the three interaction methods. API keys are stored in the operating system credential vault (Windows Credential Manager on Windows), under a configuration-specific account; they are never returned to the browser or written to the configuration file or a game save. Leave the key field blank when editing to keep the saved key. Use **Forget API key** to remove it.

DFCompanion does not make AI calls just because reports arrive. It keeps reports in local memory until you ask a question, then sends the accumulated reports and chat context to the selected provider. Review the linked policy and data-use terms before using AI features. Provider policy pages are maintained by the providers and can change.

If a request fails, the app displays the error and keeps your pending chat and reports in memory. Correct the configuration or key if needed, then use **Retry failed AI request**. The conversation and pending context are not saved when DFCompanion exits.

### Getting AI API access

AI API access is separate from using a provider's consumer chat app or subscription, and API calls may incur charges. Sign up directly with the provider you choose:

- **Anthropic:** create or sign in to an account at [Anthropic Console](https://console.anthropic.com/), review API access and billing, then create an API key in the console.
- **OpenAI:** create or sign in to an account at [OpenAI Platform](https://platform.openai.com/), review API billing and limits, then create an API key in the platform.
Google Gemini is intentionally not offered because its terms do not permit the project's intended use. The separate [`genai-rs`](https://github.com/evansenter/genai-rs) project you may encounter is a Gemini API client, not the provider-neutral `genai` crate used by DFCompanion.

Before using a key, check the provider's current pricing, free-tier limits, billing requirements, and model availability. Set any available spending limits or alerts. Keep the key private and enter it only in DFCompanion's local **API key** field; do not paste it into chat, a game save, source code, or a public issue. DFCompanion stores it in the operating system credential vault and sends it to the chosen provider only in the provider's API authentication header.

### Model suggestions

Model names, availability, prices, and capabilities change frequently. These are starting points from the providers' public catalogs as checked on **October 18, 2026**, not fixed defaults or endorsements. Check current availability and pricing in your account before use; enter the exact model ID accepted by your provider.

| Interaction method | Suggested model ID | Why consider it | Current model information |
|---|---|---|---|
| Anthropic | `claude-sonnet-5-5` | A balanced starting point for speed and capability. | [Models overview](https://platform.claude.com/docs/en/models/overview) |
| OpenAI Chat Completions | `gpt-5.4-mini` | A faster, lower-cost option suitable for trying frequent short turns. | [GPT-5.4 Mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini) |
| OpenAI Responses | `gpt-5.4-mini` | Uses OpenAI's Responses API with native tool-call handling. | [Responses API guide](https://developers.openai.com/api/docs/guides/text) |

Treat these suggestions as perishable: the provider's current catalog and your own results should take precedence.

## Current limitations

- AI calls occur only after player chat or a retry; fortress reports do not independently trigger requests.
- Input is limited to an estimated 16,000 tokens by dropping oldest pending reports first, then older conversation turns; output is capped at 4,096 tokens.
- `REPORT` is the only forwarded gameplay event type so far. The usefulness and coverage of other events have not been evaluated.
- Conversation history and pending reports are held in memory only; they are not restored after restarting the Companion.
- Enablement state is in memory; the Lua script does not persist state to the Dwarf Fortress save.
- This is a development prototype without a user installer or packaged release.

## Developer setup

Build, configure the DFHack install directory, install the Lua script, run tests, and set up VS Code using [BUILDING.md](./BUILDING.md).
