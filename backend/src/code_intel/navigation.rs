#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    SymbolId(String),
    Position(FilePositionTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePositionTarget {
    pub path: String,
    pub line: i32,
    pub col: Option<i32>,
}

pub fn parse_navigation_target(value: &str) -> Result<NavigationTarget, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("target must not be empty".to_string());
    }
    if value.starts_with("sym_") && !value.contains(':') {
        return Ok(NavigationTarget::SymbolId(value.to_string()));
    }

    let Some((path_or_path_line, final_part)) = value.rsplit_once(':') else {
        return Err("target must be a symbol id or path:line[:col]".to_string());
    };
    let final_number = parse_positive_i32(final_part, "line or column")?;
    let (path, line, col) = if let Some((path, maybe_line)) = path_or_path_line.rsplit_once(':') {
        if let Ok(line) = parse_positive_i32(maybe_line, "line") {
            (path, line, Some(final_number))
        } else {
            (path_or_path_line, final_number, None)
        }
    } else {
        (path_or_path_line, final_number, None)
    };

    let path = path.trim();
    if path.is_empty() {
        return Err("target path must not be empty".to_string());
    }
    Ok(NavigationTarget::Position(FilePositionTarget {
        path: path.to_string(),
        line,
        col,
    }))
}

pub fn identifier_at_position(source: &str, line: i32, col: Option<i32>) -> Option<String> {
    let col = col?;
    if line < 1 || col < 1 {
        return None;
    }
    let (line_start, line_end) = line_bounds(source, line as usize)?;
    if line_start >= line_end {
        return None;
    }

    let cursor = line_start.saturating_add((col - 1) as usize);
    let bytes = source.as_bytes();
    let mut pos = cursor.min(line_end.saturating_sub(1));
    if !is_identifier_byte(bytes[pos]) && pos > line_start && is_identifier_byte(bytes[pos - 1]) {
        pos -= 1;
    }
    if !is_identifier_byte(bytes[pos]) {
        return None;
    }

    let mut start = pos;
    while start > line_start && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = pos + 1;
    while end < line_end && is_identifier_byte(bytes[end]) {
        end += 1;
    }
    source.get(start..end).map(str::to_string)
}

pub fn first_identifier_column_on_line(source: &str, line: i32) -> Option<i32> {
    if line < 1 {
        return None;
    }
    let (line_start, line_end) = line_bounds(source, line as usize)?;
    let bytes = source.as_bytes();
    for (idx, byte) in bytes.iter().enumerate().take(line_end).skip(line_start) {
        if is_identifier_start_byte(*byte) {
            return Some((idx - line_start + 1) as i32);
        }
    }
    None
}

fn parse_positive_i32(value: &str, label: &str) -> Result<i32, String> {
    let parsed = value
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("target {label} must be a positive integer"))?;
    if parsed < 1 {
        return Err(format!("target {label} must be a positive integer"));
    }
    Ok(parsed)
}

fn line_bounds(source: &str, target_line: usize) -> Option<(usize, usize)> {
    let mut line = 1;
    let mut start = 0;
    for (idx, byte) in source.bytes().enumerate() {
        if line == target_line && byte == b'\n' {
            return Some((start, idx));
        }
        if byte == b'\n' {
            line += 1;
            start = idx + 1;
        }
    }
    if line == target_line {
        Some((start, source.len()))
    } else {
        None
    }
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn is_identifier_start_byte(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_symbol_and_position_targets() {
        assert_eq!(
            parse_navigation_target("sym_abc123").unwrap(),
            NavigationTarget::SymbolId("sym_abc123".to_string())
        );
        assert_eq!(
            parse_navigation_target("backend/src/lib.rs:42").unwrap(),
            NavigationTarget::Position(FilePositionTarget {
                path: "backend/src/lib.rs".to_string(),
                line: 42,
                col: None,
            })
        );
        assert_eq!(
            parse_navigation_target("backend/src/lib.rs:42:7").unwrap(),
            NavigationTarget::Position(FilePositionTarget {
                path: "backend/src/lib.rs".to_string(),
                line: 42,
                col: Some(7),
            })
        );
        assert!(parse_navigation_target("backend/src/lib.rs").is_err());
    }

    #[test]
    fn extracts_identifier_at_one_based_position() {
        let source = "fn main() {\n    render_widget(value);\n}\n";

        assert_eq!(
            identifier_at_position(source, 2, Some(8)).as_deref(),
            Some("render_widget")
        );
        assert_eq!(
            identifier_at_position(source, 2, Some(21)).as_deref(),
            Some("value")
        );
        assert_eq!(identifier_at_position(source, 2, Some(4)), None);
        assert_eq!(identifier_at_position(source, 2, None), None);
        assert_eq!(first_identifier_column_on_line(source, 2), Some(5));
    }
}
