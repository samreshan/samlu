//! One-line, human-readable description of a tool call for approval cards.

pub fn action_line(tool_name: &str, tool_input: &serde_json::Value) -> String {
    let _ = (tool_name, tool_input);
    String::new()
}

/// "Wants to run" | "Wants to edit" | "Needs approval"
pub fn approval_title(tool_name: &str) -> &'static str {
    let _ = tool_name;
    "Needs approval"
}
