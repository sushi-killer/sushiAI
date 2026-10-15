use crate::engine::permissions::tools::TaskTools;
use crate::prompts::engine_text;

/// The block the implement and plan briefs carry.
pub(in crate::engine) fn tools_block(tools: &TaskTools, planning: bool) -> String {
    tools_block_text(&tools.lines(), &tools.off, planning)
}

/// [`tools_block`] over the server `lines` and the connected tools that are
/// `off` as `(id, label)`.
pub(in crate::engine) fn tools_block_text(
    lines: &[String],
    off: &[(String, String)],
    planning: bool,
) -> String {
    let mut text = format!("{}\n", engine_text("permissions_tools_header"));
    if lines.is_empty() {
        text.push_str("- none\n");
    }
    for line in lines {
        text.push_str(&format!("- {line}\n"));
    }
    for (id, label) in off {
        text.push_str(&format!(
            "- {id} ({label}): connected but off for this repo\n"
        ));
    }
    if planning {
        text.push_str(&format!(
            "\n{}\n\n",
            engine_text("permissions_tools_planning")
        ));
    } else {
        text.push_str(&format!(
            "\n{}\n\n",
            engine_text("permissions_tools_implementing")
        ));
    }
    text
}
