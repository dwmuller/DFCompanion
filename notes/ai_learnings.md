# Notes on working with AI

Since this project represents my first dive into writing program that interact
with AI models, and my first time programming with non-trivial assistance from
AIs, I'm keeping notes on the process and lessons learned.

I look at this from the PoV of a former professional developer who now
approaches it as a cherished hobby.

## Tool-call context growth

An AI tool turn is not one fixed request. The assistant's tool-use message and
the corresponding tool results are appended to the request before each later
round, so repeated tool calls can grow the within-turn input dramatically even
when the initial prompt is under budget.

The output allowance also applies to intermediate model responses. A model can
use it for reasoning or further tool calls and then return no final text.

One observed failure expanded from an estimated 17,000-token initial request to
an estimated 157,366 tokens after five tool-call batches, then consumed the
entire then-available 4,096-token output allowance without producing a final
answer. Most of the growth came from accumulated tool-call/result context.

Design responses should therefore include:

- compact tool outputs and pagination;
- a per-turn tool-call budget;
- purpose-built tools that avoid forcing models to infer a simple fact through
  many broad detail calls; and
- enough observability to distinguish initial-prompt usage from later
  tool-round usage.

## Commercial API access

The market is burgeoning and complex. All I can touch on here is what I've
learned through AI-assisted Web searches and personal experience. Details are
changing constantly, so don't expect many here.

There are many providers with a variety of models available. Access to
proprietary models are, of course, available only through the model owners, and
the frontier models are not cheap to use. Pricing strategies vary, but most
providers list prices per $1M input tokens, output tokens, cached tokens read,
and cached tokens written. Some providers offer a free tier but it's usually
pretty limited.

Hugging Face stands out a bit in my experience so far (2026-11-13), because they
charge for uptime of a dedicated VM instead. That's kind of a difficult pricing
model for hobby usage.

## Multi-provider gateway

Just found out about these today (2026-11-13), in connection with the VSCode
Cline extension. Examples are Cline and OpenRouter. Basically, you choose the
model you want and they route requests to a provider.

One potential advantage is that if you pay through one of these services, you
only have to manage an account and API keys for that provider. If you do this,
OpenRouter charges a 5.5% fee. Presumably CLine works similarly in this mode.
Since I had three accounts with token balances by the time I noticed these
services, this seems attractive.

Other services of this sort worthy of note, from a query:

- Hosted, one key, many models.
    - Requesty
    - NanoGPT
    - Together AI
    - AI/ML API
- Self-hosted (you run the gateway).
    - LiteLLM
    - Bitfrost
    - Portkey

### Cline

Offers ClinePass, a subscription-based service that sounds affordable for medium
to heavy hobby users. In usage-billing mode, you pay through Cline, and
presumably they take a small cut. This uses a small number of actual compute
providers that are opaque to you.

### OpenRouter

They have two [pricing models](https://openrouter.ai/pricing) of interest for
hobby work: Free and Standard. Standard is pay-as-you-go, with OpenRouter adding
5.5% of what otherwise passes through to the providers. If you bring your own
API keys for providers, then you don't pay OpenRouter at all (up to a monthly
limit that's far too high to be a concern for hobby programmers). If the
provider's key runs out of credits, it falls back to charging your OpenRouter
balance -- a feature you can supposedly turn off per-key.


## Tools for working with AI

### Web-based options

I discovered that I could ask Windows Copilot questions about my project and
because it could read the pushed files from GitHub. (More on this below.) Later
I discovered that Claude could do this too -- it even pulls a working copy of
the project to examine. And *then* I discovered that [Cline can even make
changes and submit them as a branch or a pull request](https://cline.bot/cloud).
So you can actually get started at AI-assisted development without dealing with
IDE integration at all!

### VSCode built-in Copilot Harness

This works right out of the box up to a point, and beyond that point if you get
a subscription and possibly pay for additional tokens. I found it a bit
expensive to use, but it was a great tool for getitng started.

I found ways of reducing cost by turning to Windows Copilot for expensive
strategizing, and having it build clear instructions for the in-IDE Copilot to
follow.

This worked by having Windows Copilot read the associated public Github repo --
it can't examine a workspace on your drive. But for some reason the Github MCP
connector started breaking in some sessions, and then stopped working entirely,
and I wasn't able to get it working again. (2026-11-10 through 13).

This problem, and the somewhat high cost of using Copilot, drove me to start
experimenting with alternatives.

### VSCode Cline extension

Trying this out now. (2026-11-13) 

Cline also has a desktop app similar to Claude Code (which I've not used). The
use cases for this don't sound applicable to my hobby usage, though it has some
nice features that are missing from the extension. In particular, if you're
bringing your own provider key to the extension, you can only use one provider
at a time and have to switch when changing models. This is what led to me
discovery of AI routers. OpenRouter provides a workaround for this limitation,
if you're not ready to use a ClinePass (subscription) or Cline usage-billing.


### VSCode Continue extension

Tried this out and was not thrilled with the UI. Had several hours into it
before I ran across the fact that it's no longer actively updated. (2026-11-12)

## Things I tried that didn't work.

Both the Continue and Cline extensions were disatisfying. Despite various
attempts to improve their configuration, in these harnesses the AIs insisted on
using command line tools to examine source files in various ways. Cline also
seemed to have trouble managing and retrieving output from console sessions.
Both seemed to get stuck in loops that consumed tokens at a frightening rate.

Installing agent-lsp to get Cline to use MCP server tools that rely on language
service providers instead of CLI searches had some salutory effect, but it
wasn't enough to fix the problems.

With Cline I got far enough along to try customizing its "rules". I later had an
AI transform these to Github harness rules.

## What I'm doing now

I've returned to using the built-in Copilot agent harness, configured to use
OpenRouter with help from Google's search AI, which generally gives better
answers to technical question than the Brave search AI. (Brave is my default
browser, and Brave search is its default search engine.)

The built-in harness seems to work with the project files much, much more
efficiently, using the same models I was using with Cline. (Claude Sonnet 5 for
agentic actions. )