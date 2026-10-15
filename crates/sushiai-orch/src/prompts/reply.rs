/// The JSON a reply hands back: the last ```tag (or <tag>) block, else the
/// outermost `{...}` -- Codex has been seen to drop the backticks and send
/// the bare tag line followed by the JSON.
pub(crate) fn tagged_json(text: &str, tag: &str) -> Option<String> {
    last_fenced_block(text, tag).or_else(|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        (start < end).then(|| text[start..=end].to_string())
    })
}

/// The body of the last ` ```<tag>` fence or `<tag>...</tag>` block in
/// `text`, whichever opens later. A fence closes only on a line that is
/// just ` ``` `, so a JSON string quoting ` ```sushi-review``` ` inline
/// doesn't cut the body short. Models use both shapes, so either counts.
pub(crate) fn last_fenced_block(text: &str, tag: &str) -> Option<String> {
    last_tagged_block(text, tag).map(|block| block.body.to_string())
}

pub(crate) struct TaggedBlock<'a> {
    pub(crate) body: &'a str,
    span: std::ops::Range<usize>,
}

impl TaggedBlock<'_> {
    pub(crate) fn remove_from(&self, text: &str) -> String {
        let before = text[..self.span.start].trim_end();
        let after = text[self.span.end..].trim_start();
        [before, after]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub(crate) fn last_tagged_block<'a>(text: &'a str, tag: &str) -> Option<TaggedBlock<'a>> {
    let fence = format!("```{tag}");
    let open_tag = format!("<{tag}>");
    let fence_at = text.rfind(&fence);
    let tag_at = text.rfind(&open_tag);
    if tag_at.is_some() && (fence_at.is_none() || tag_at > fence_at) {
        let start = tag_at?;
        let content = start + open_tag.len();
        let close_tag = format!("</{tag}>");
        let close = content + text[content..].find(&close_tag)?;
        return Some(TaggedBlock {
            body: text[content..close].trim(),
            span: start..close + close_tag.len(),
        });
    }
    let start = fence_at?;
    let after_open = start + fence.len();
    let content = after_open + text[after_open..].find('\n')? + 1;
    let mut offset = content;
    for line in text[content..].split_inclusive('\n') {
        if line.trim() == "```" {
            return Some(TaggedBlock {
                body: text[content..offset].trim(),
                span: start..offset + line.len(),
            });
        }
        offset += line.len();
    }
    // Some replies close the fence on the JSON's own line.
    let close = content + text[content..].rfind("```")?;
    Some(TaggedBlock {
        body: text[content..close].trim(),
        span: start..close + 3,
    })
}

/// A handoff the agent wrote as null or as an object/array still reads:
/// null is empty, structure is kept as its JSON text.
pub(crate) fn lenient_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    })
}
