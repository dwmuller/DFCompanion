# Notes on working with AI

Since this project represents my first dive into writing program that interact
with AI models, and my first time programming with non-trivial assistance from
AIs, I'm keeping notes on the process and lessons learned.

I look at this from the PoV of a former professional developer who now
approaches it as a cherished hobby.

## Commercial API access

There are many providers with a variety of models available. Access to
proprietary models are, of course, available only through the model owners, and
the frontier models are not cheap to use. Pricing strategies vary, but most
providers list prices per $1M input tokens, output tokens, cached tokens read,
and cached tokens written. Some providers offer a free tier but it's usually
pretty limited.

Hugging Face stands out a bit in my experience so far (2026-11-13), because they
charge for uptime of a dedicated VM instead. That's kind of a difficult pricing
model for hobby usage.

## Multi-provider routers

Just found out about these today (2026-11-13), in connection with the VSCode
Cline extension. Examples are Cline and OpenRouter. Basically, you choose the
model you want and they route requests to a provider.

One potential advantage is that if you pay through one of these services, you
only have to manage an account and API keys for that provider. If you do this,
OpenRouter charges a 5.5% fee. Presumably CLine works similarly in this mode.
Since I had three accounts with token balances by the time I noticed these
services, this seems attractive.

### Cline

Offers ClinePass, a subscription-based service that sounds affordable for medium
to heavy hobby users. In usage-billing mode, you pay through Cline, and
presumably they take a small cut.

### OpenRouter

They have two [pricing models](https://openrouter.ai/pricing) of interest for
hobby work: Free and Standard. Standard is pay-as-you-go, with OpenRouter adding
5.5% of what otherwise passes through to the providers. If you bring your own
API keys for providers, then you don't pay OpenRouter at all (up to a monthly
limit that's far too high to be a concern for hobby programmers). If the
provider's key runs out of credits, it falls back to charging your OpenRouter
balance -- a feature you can supposedly turn off per-key.


## Tools for working with AI

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
if you're not ready to use a ClinePass (subscription) or Cline usage-billing --
e.g. if you have several provider keys with associated token balances already.


### VSCode Continue extension

Tried this out and was not thrilled with the UI. Had several hours into it
before I ran across the fact that it's no longer actively updated. (2026-11-12)

