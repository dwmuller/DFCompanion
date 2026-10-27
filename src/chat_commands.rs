//! Slash commands typed into the chat. They arrive through the same queue as
//! ordinary user messages and are recognized and run by the companion worker
//! instead of being sent to the AI.

use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum ChatCommand {
    /// Runs an AI tool directly, for manual testing.
    CallTool { name: String, arguments: Value },
    /// Lists the AI tools and their arguments.
    ListTools,
    /// Describes one AI tool in full.
    DescribeTool { name: String },
}

impl ChatCommand {
    /// Returns `None` when the text is not a command, so it is an ordinary
    /// message for the AI.
    ///
    /// `/call <tool> [arguments]` takes either a JSON object (`{"limit": 5}`)
    /// or whitespace-separated `key=value` pairs (`limit=5 offset=10`). Each
    /// value is read as JSON when it is valid JSON (numbers, `true`, `null`,
    /// quoted strings) and as plain text otherwise. Values containing spaces
    /// must be quoted (`race="magma crab"` or `race='magma crab'`).
    pub fn parse(text: &str) -> Option<Result<Self, String>> {
        let rest = text.trim_start().strip_prefix('/')?;
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return None;
        }
        let (command, rest) = split_word(rest);
        Some(match command {
            "call" => parse_call(rest),
            "tools" if rest.is_empty() => Ok(ChatCommand::ListTools),
            "tools" => Err("Usage: /tools".to_owned()),
            "tool" if !rest.is_empty() && !rest.contains(char::is_whitespace) => {
                Ok(ChatCommand::DescribeTool {
                    name: rest.to_owned(),
                })
            }
            "tool" => Err("Usage: /tool <tool name>".to_owned()),
            // Add more chat commands here as needed.
            _ => Err(format!("Unknown chat command: /{command}")),
        })
    }
}

fn split_word(text: &str) -> (&str, &str) {
    match text.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim()),
        None => (text, ""),
    }
}

fn parse_call(text: &str) -> Result<ChatCommand, String> {
    let (name, arguments) = split_word(text);
    if name.is_empty() {
        return Err("Usage: /call <tool> [key=value ... | {json arguments}]".to_owned());
    }
    let arguments = parse_call_arguments(arguments)?;
    Ok(ChatCommand::CallTool {
        name: name.to_owned(),
        arguments,
    })
}

/// Splits on whitespace that is not inside single or double quotes. Quotes are
/// kept in the tokens; a backslash escapes the next character inside quotes.
fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == '\\' {
                    current.extend(chars.next());
                } else if c == q {
                    quote = None;
                }
            }
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
                current.push(c);
            }
        }
    }
    if quote.is_some() {
        return Err("Arguments contain an unterminated quote.".to_owned());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

fn parse_value(value: &str) -> Value {
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Value::String(inner.to_owned());
    }
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()))
}

fn parse_call_arguments(text: &str) -> Result<Value, String> {
    if text.is_empty() {
        return Ok(Value::Null);
    }
    if text.starts_with('{') {
        return serde_json::from_str(text)
            .map_err(|error| format!("Arguments are not valid JSON: {error}"));
    }
    let mut object = serde_json::Map::new();
    for pair in split_arguments(text)? {
        let (key, value) = pair
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| format!("Expected key=value but found \"{pair}\"."))?;
        if object.insert(key.to_owned(), parse_value(value)).is_some() {
            return Err(format!("Argument \"{key}\" was given more than once."));
        }
    }
    Ok(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> Result<ChatCommand, String> {
        Ok(ChatCommand::CallTool {
            name: name.to_owned(),
            arguments,
        })
    }

    #[test]
    fn ordinary_text_is_not_a_command() {
        assert!(ChatCommand::parse("hello").is_none());
        assert!(ChatCommand::parse("/ call x").is_none());
        assert!(ChatCommand::parse("/").is_none());
    }

    #[test]
    fn unknown_commands_and_missing_tool_names_are_errors() {
        assert!(ChatCommand::parse("/caller x").unwrap().is_err());
        assert!(ChatCommand::parse("/call").unwrap().is_err());
    }

    #[test]
    fn call_arguments_may_be_key_value_pairs_or_json() {
        assert_eq!(
            ChatCommand::parse("/call list_fortress_units").unwrap(),
            call("list_fortress_units", Value::Null)
        );
        assert_eq!(
            ChatCommand::parse("/call t limit=5 offset=10 name=bob q=\"a\" ok=true").unwrap(),
            call(
                "t",
                json!({"limit": 5, "offset": 10, "name": "bob", "q": "a", "ok": true})
            )
        );
        assert_eq!(
            ChatCommand::parse("/call t  {\"a\": 1}").unwrap(),
            call("t", json!({"a": 1}))
        );
    }

    #[test]
    fn tools_command_takes_no_arguments() {
        assert_eq!(
            ChatCommand::parse("/tools"),
            Some(Ok(ChatCommand::ListTools))
        );
        assert!(ChatCommand::parse("/tools x").unwrap().is_err());
        assert_eq!(
            ChatCommand::parse("/tool get_unit_details"),
            Some(Ok(ChatCommand::DescribeTool {
                name: "get_unit_details".to_owned()
            }))
        );
        assert!(ChatCommand::parse("/tool").unwrap().is_err());
        assert!(ChatCommand::parse("/tool a b").unwrap().is_err());
    }

    #[test]
    fn quoted_values_may_contain_spaces() {
        assert_eq!(
            ChatCommand::parse("/call t race=\"magma crab\" limit=5").unwrap(),
            call("t", json!({"race": "magma crab", "limit": 5}))
        );
        assert_eq!(
            ChatCommand::parse("/call t race='magma crab' a=\"x \\\"y\\\" z\"").unwrap(),
            call("t", json!({"race": "magma crab", "a": "x \"y\" z"}))
        );
        assert!(ChatCommand::parse("/call t race=\"magma crab")
            .unwrap()
            .is_err());
    }

    #[test]
    fn malformed_call_arguments_are_errors() {
        for text in [
            "/call t limit",
            "/call t =5",
            "/call t a=1 a=2",
            "/call t {oops",
        ] {
            assert!(ChatCommand::parse(text).unwrap().is_err(), "{text}");
        }
    }
}
