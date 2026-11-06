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
