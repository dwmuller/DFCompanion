# Analyses

Almost all of the core code was developed by GitHub Copilot. Here you'll find
records of human analyses to help understand how things flow.

## Flow analysis of a successful AI turn

- lib::try_ai_turn()
  - runs once for a periodic check, or loops while there are pending user
    messages in Conversation
    - ai::Conversation::build_prompt() -- returns trimmed lib::Prompt
    - ai::Runtime::complete(configuration, prompt) defers to
      ai::Runtime::complete_with_key()
      - returns AiTurnResult
      - uses genai to send request and tool results, looping until there are no
        more tool requests
    - ai::Conversation::complete(prompt, result.answer :String)
    - returns nothing, records history, clears reports

### Details of loop in ai::Runtime::complete_with_key()

For compatibility with Anthropic-style tool-use APIs, DFCompanion/genai collects
all results from a batch of tool calls into the single tool-result message
following the assistant's tool-use message.

This is where much of the interaction with genai occurs.

- A ChatRequest is sent at the top of the loop, resulting in a ChatResponse.
- The response provides a list of ToolCall and an assistant msg as a
  ChatMessage.
- This assistant ChatMessage is appended to the ChatRequest for the next loop
  execution.
- Each ToolCall is executed via ai_tools::execute() resulting in the tool output
  as a String (or an error string).
- These strings are turned into genai ToolResponse objects which are collected
  in a vector.
- genai tools are used to convert these ToolResponses to a single ChatMessage
  which is appended to the ChatRequest, and the loop repeats.
- Tool outputs are not saved anywhere else -- they don't become part of the
  conversation history.

Each tool call batch results in the chat request growing by one assistant
message and all the responses.

### Chat growth

Instrumentation of failures due to violation of MAX_OUTPUT_TOKENS led to the
discovery of two related problems: intermediate reasoning could consume the
entire output allowance, while repeated tool calls and their results could cause
very large within-turn growth of the input context. One failure involved an
initial request that engendered five tool-call batches:

```
Tokens for initial request:
   17000 est,  19078 input (  5760 [30%] cached,      0 cache creation),    358 output,   19436 total [114%]
Tokens for tool call(s) list_locations, get_fortress_overview:
   18129 est,  19985 input ( 19328 [96%] cached,      0 cache creation),    460 output,   20445 total [112%]
Tokens for tool call(s) list_residents, list_residents, list_residents:
   28578 est,  29868 input ( 20352 [68%] cached,      0 cache creation),   1540 output,   31408 total [109%]
Tokens for tool call(s) get_unit_details, get_unit_details, get_unit_details, get_unit_details, get_unit_details, get_unit_details:
   99335 est,  89806 input ( 31360 [34%] cached,      0 cache creation),   1429 output,   91235 total [91%]
Tokens for tool call(s) get_unit_details, get_unit_details, get_unit_details, get_unit_details, get_unit_details, list_zones:
  148375 est, 130703 input ( 91136 [69%] cached,      0 cache creation),   3828 output,  134531 total [90%]
Tokens for tool call(s) get_unit_details, get_unit_details:
  157366 est, 139507 input (134528 [96%] cached,      0 cache creation),   4096 output,  143603 total [91%]
```

By far most of the growth in input tokens was caused by accumulated
tool-call/result context. In the final request, the AI then consumed the entire
4,096-token output allowance in intermediate reasoning without reaching a final
text response.

The excessive tool activity seems to have been caused at least in part by a lack
of tools suitable to the task the AI was working on. In particular, it
repeatedly called get_unit_details to infer religious affiliation from indirect
data because explicit religion/sect membership was not available through the
tools.
