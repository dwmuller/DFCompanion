# Dwarf Fortress AI Companion

DFCompanion is an early-stage companion for Dwarf Fortress. It connects DFHack observations and player questions in a local web app, with the goal of helping an AI answer questions about a live fortress while leaving control of the game to the player.

This is an experimental prototype, not yet a packaged release. It can send chat turns to Anthropic or an OpenAI-compatible API after you configure an API key, model ID, and privacy acknowledgement.

## Getting started

The current Lua event prototype has been tested with Dwarf Fortress 53.16 and DFHack 53.16-r2 on Windows. You need a local DFHack installation and a DFCompanion build. See [BUILDING.md](./BUILDING.md) for developer setup and instructions to build and install the Lua script.

1. Start DFCompanion. The local web app is at <http://127.0.0.1:3000/>.
2. Copy the `dfcompanion <token>` command shown on the web page.
3. With a fortress loaded in Dwarf Fortress, paste that command into the DFHack console. This enables the script and sends a connection-test event.
4. Configure AI access in the web app's **AI Provider** section (see [Getting AI API access](#getting-ai-api-access)).
5. Review the selected provider's privacy policy, acknowledge the data-sharing notice, and save the settings.
6. Ask a question in the web app's Chat box. Reports received since the previous AI answer are sent along with your question.

The generated token changes every time DFCompanion starts. Copy the new command from the web page after restarting the service. Do not share the token: it authorizes local Lua event submissions.

## Using the DFHack script

When enabled, `dfcompanion.lua` listens for DFHack `REPORT` events and forwards report text to DFCompanion. Announcements and other reports are labeled separately. DFHack reports occur more often than visible announcements, so the script may send many events. Forwarding is synchronous.

- `disable dfcompanion` stops forwarding reports.
- `enable dfcompanion` re-enables it during the current DFHack session.
- `dfcompanion status` displays its current in-memory state.

The enabled state is not saved; it resets when the map unloads or DFHack exits. If the bounded event queue fills, the script pauses the game and reports the condition. If forwarding fails, the script disables itself and reports the error.

## AI providers and privacy

Choose Anthropic, OpenAI, Groq, or a custom OpenAI-compatible API in the **AI Provider** section of the web app. OpenAI and Groq use their known API endpoints; for a custom service, enter its HTTPS API base URL (usually ending in `/v1`). Changing providers selects a suggested starting model ID where one is available; the model field remains free-form so you can enter the exact ID accepted by your service. Provider, model, and custom endpoint settings are saved in your per-user local application data. API keys are saved in the operating system's credential vault (Windows Credential Manager on Windows), not in the settings file or a game save. Custom API keys are associated with the configured base URL. Use **Forget saved API key** to remove the selected endpoint's key.

OpenAI and Groq are handled through the same OpenAI-compatible chat-completions adapter. Other services can be configured if they implement a compatible `POST /chat/completions` endpoint with bearer-token authentication. Compatibility varies; some services may not support every request option or response shape. For custom services, check the service's privacy and data-use terms yourself; DFCompanion cannot identify or link a custom endpoint's policies.

DFCompanion does not make AI calls just because reports arrive. It keeps reports in local memory until you ask a question, then sends the accumulated reports and chat context to the selected provider. Review the linked policy and acknowledge the notice before the first request; changing providers or the notice version requires acknowledgement again. Provider policy pages are maintained by the providers and can change.

If a request fails, the app displays the error and keeps your pending chat and reports in memory. Correct the provider/model/key settings if needed, then use **Retry failed AI request**. The conversation and pending context are not saved when DFCompanion exits.

### Getting AI API access

AI API access is separate from using a provider's consumer chat app or subscription, and API calls may incur charges. Sign up directly with the provider you choose:

- **Anthropic:** create or sign in to an account at [Anthropic Console](https://console.anthropic.com/), review API access and billing, then create an API key in the console.
- **OpenAI:** create or sign in to an account at [OpenAI Platform](https://platform.openai.com/), review API billing and limits, then create an API key in the platform.
- **Groq:** create or sign in to an account at [Groq Console](https://console.groq.com/), review its API and usage limits, then create an API key. Groq's OpenAI-compatible API uses the preset base URL in DFCompanion.

For a custom OpenAI-compatible service, obtain its API key, base URL, and exact model ID from that service, then select **Custom OpenAI-compatible API** and enter them in the web app. Google Gemini is intentionally not offered because its terms do not permit the project's intended use.

Before using a key, check the provider's current pricing, free-tier limits, billing requirements, and model availability. Set any available spending limits or alerts. Keep the key private and enter it only in DFCompanion's local **API key** field; do not paste it into chat, a game save, source code, or a public issue. DFCompanion stores it in the operating system credential vault and sends it to the chosen provider only in the provider's API authentication header.

### Model suggestions

Model names, availability, prices, and capabilities change frequently. These are starting points from the providers' public catalogs as checked on **October 18, 2026**, not fixed defaults or endorsements. Check current availability and pricing in your account before use; enter the exact model ID accepted by your provider.

| Provider | Suggested model ID | Why consider it | Current model information |
|---|---|---|---|
| Anthropic | `claude-sonnet-5-5` | A balanced starting point for speed and capability. | [Models overview](https://platform.claude.com/docs/en/models/overview) |
| OpenAI | `gpt-5.4-mini` | A faster, lower-cost option suitable for trying frequent short turns. | [GPT-5.4 Mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini) |
| Groq | No fixed suggestion | Select a model currently available to your Groq account. | [Groq model catalog](https://console.groq.com/docs/models) |
| Custom OpenAI-compatible API | No fixed suggestion | Use a model ID documented by the selected service. | Consult that service's model catalog. |

Treat these suggestions as perishable: the provider's current catalog and your own results should take precedence.

## Current limitations

- AI calls occur only after player chat or a retry; fortress reports do not independently trigger requests.
- Input is limited to an estimated 16,000 tokens by dropping oldest pending reports first, then older conversation turns; output is capped at 512 tokens.
- `REPORT` is the only forwarded gameplay event type so far. The usefulness and coverage of other events have not been evaluated.
- Conversation history and pending reports are held in memory only; they are not restored after restarting the Companion.
- Enablement state is in memory; the Lua script does not persist state to the Dwarf Fortress save.
- This is a development prototype without a user installer or packaged release.

## Developer setup

Build, configure the DFHack install directory, install the Lua script, run tests, and set up VS Code using [BUILDING.md](./BUILDING.md).
