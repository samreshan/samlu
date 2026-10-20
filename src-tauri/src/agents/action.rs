//! One-line, human-readable description of a tool call for approval cards.

/// Input fields to show, in priority order. The first non-empty one wins.
const DETAIL_FIELDS: [&str; 7] = [
    "command",
    "file_path",
    "notebook_path",
    "url",
    "pattern",
    "path",
    "description",
];

/// Longest text a card, its tooltip or a banner will carry. The line itself
/// is kept whole so the user can read what they approve; this only stops a
/// huge command from flooding the screen. History cuts shorter (160).
const MAX_CARD_CHARS: usize = 4000;

pub fn action_line(tool_name: &str, tool_input: &serde_json::Value) -> String {
    let detail = DETAIL_FIELDS.iter().find_map(|key| {
        tool_input
            .get(*key)
            .and_then(|value| value.as_str())
            .map(collapse)
            .filter(|text| !text.is_empty())
    });
    detail.unwrap_or_else(|| collapse(tool_name))
}

/// `action` cut for display on a card, tooltip or banner.
pub fn card_text(action: &str) -> String {
    truncate(action.to_string())
}

/// "Wants to run" | "Wants to edit" | "Needs approval"
pub fn approval_title(tool_name: &str) -> &'static str {
    match tool_name {
        "Bash" | "exec" | "shell" | "local_shell" => "Wants to run",
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch" => "Wants to edit",
        _ => "Needs approval",
    }
}

/// Joins all whitespace runs, including newlines, into single spaces.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_CARD_CHARS {
        return text;
    }
    let mut short: String = text.chars().take(MAX_CARD_CHARS - 1).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_uses_command() {
        assert_eq!(
            action_line(
                "Bash",
                &json!({"command":"curl -sI https://example.com","description":"x"})
            ),
            "curl -sI https://example.com"
        );
    }

    #[test]
    fn edit_uses_file_path() {
        assert_eq!(
            action_line("Edit", &json!({"file_path":"/p/src/app.ts"})),
            "/p/src/app.ts"
        );
    }

    #[test]
    fn multiline_command_is_one_line() {
        assert_eq!(action_line("Bash", &json!({"command":"a\n  b"})), "a b");
    }

    #[test]
    fn falls_back_to_description_then_tool() {
        assert_eq!(
            action_line("WebFetch", &json!({"url":"https://x.dev"})),
            "https://x.dev"
        );
        assert_eq!(
            action_line("Bash", &json!({"description":"List files"})),
            "List files"
        );
        assert_eq!(action_line("mcp__x__y", &json!({})), "mcp__x__y");
    }

    #[test]
    fn long_lines_are_kept_whole_and_cut_only_for_display() {
        let long = "x".repeat(400);
        let line = action_line("Bash", &json!({ "command": long }));
        assert_eq!(line.chars().count(), 400, "the action line is not cut");
        assert_eq!(card_text(&line), line);

        let huge = "y".repeat(5000);
        let line = action_line("Bash", &json!({ "command": huge }));
        assert_eq!(line.chars().count(), 5000);
        let shown = card_text(&line);
        assert_eq!(shown.chars().count(), MAX_CARD_CHARS);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn titles() {
        assert_eq!(approval_title("Bash"), "Wants to run");
        assert_eq!(approval_title("local_shell"), "Wants to run");
        assert_eq!(approval_title("Write"), "Wants to edit");
        assert_eq!(approval_title("apply_patch"), "Wants to edit");
        assert_eq!(approval_title("WebFetch"), "Needs approval");
    }
}
