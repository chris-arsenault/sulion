/// Remove paired, standalone harness paste delimiters. Literal markup in
/// fenced code, inline tags, unmatched delimiters, and the pasted body survive.
pub(super) fn normalize_user_text(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut remove = vec![false; lines.len()];
    let mut fence: Option<char> = None;
    let mut opening: Option<(usize, String)> = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().next().unwrap();
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some((start, id)) = &opening {
            if trimmed == format!("</pasted_content id=\"{id}\">") {
                remove[*start] = true;
                remove[index] = true;
                opening = None;
            }
        } else if let Some(id) = trimmed
            .strip_prefix("<pasted_content id=\"")
            .and_then(|s| s.strip_suffix("\">"))
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            opening = Some((index, id.to_owned()));
        }
    }
    if !remove.iter().any(|removed| *removed) {
        return text.to_owned();
    }
    lines.into_iter().enumerate()
        .filter_map(|(i, line)| (!remove[i]).then_some(line))
        .collect::<String>()
        .trim_matches(['\r', '\n'])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_body_surrounding_text_and_multiple_pastes() {
        let text = "Before\n<pasted_content id=\"4e43\">\n  indented body\n</pasted_content id=\"4e43\">\nBetween\n<pasted_content id=\"8b80\">\nsecond\n</pasted_content id=\"8b80\">\nAfter";
        assert_eq!(normalize_user_text(text), "Before\n  indented body\nBetween\nsecond\nAfter");
        assert_eq!(normalize_user_text("\n\n<pasted_content id=\"a\">\nhello\n</pasted_content id=\"a\">\n"), "hello");
    }

    #[test]
    fn preserves_literal_markup_and_unpaired_delimiters() {
        for text in [
            "```xml\n<pasted_content id=\"a\">\nhello\n</pasted_content id=\"a\">\n```",
            "Use <pasted_content id=\"a\">inline</pasted_content id=\"a\">",
            "<pasted_content id=\"a\">\nhello\n</pasted_content id=\"b\">",
            "<user_input>hello</user_input>",
        ] {
            assert_eq!(normalize_user_text(text), text);
        }
    }
}
