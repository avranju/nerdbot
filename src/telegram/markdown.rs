//! Markdown parser and escaping for Telegram's MarkdownV2 format.
//!
//! Telegram's MarkdownV2 is extremely strict and requires escaping most special
//! characters outside of active formatting blocks. This module parses standard
//! Markdown and encodes it safely into Telegram-compatible MarkdownV2.

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Text(String),
    Bold(Vec<Node>),
    Italic(Vec<Node>),
    Underline(Vec<Node>),
    Strikethrough(Vec<Node>),
    CodeBlock { lang: String, code: String },
    InlineCode(String),
    Link { text: Vec<Node>, url: String },
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        if self.pos < self.chars.len() {
            Some(self.chars[self.pos])
        } else {
            None
        }
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        if self.pos + n < self.chars.len() {
            Some(self.chars[self.pos + n])
        } else {
            None
        }
    }

    fn next(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        let s_chars: Vec<char> = s.chars().collect();
        if self.pos + s_chars.len() > self.chars.len() {
            return false;
        }
        for (i, c) in s_chars.iter().enumerate() {
            if self.chars[self.pos + i] != *c {
                return false;
            }
        }
        true
    }

    fn consume_str(&mut self, s: &str) {
        self.pos += s.chars().count();
    }

    fn parse_code_block(&mut self) -> Node {
        self.consume_str("```");
        // Read language if any
        let mut lang = String::new();
        while let Some(c) = self.peek() {
            if c == '\n' {
                self.next(); // consume newline
                break;
            }
            lang.push(self.next().unwrap());
        }
        let lang = lang.trim().to_string();

        // Read code content
        let mut code = String::new();
        while !self.eof() {
            if self.starts_with("```") {
                self.consume_str("```");
                break;
            }
            code.push(self.next().unwrap());
        }
        Node::CodeBlock { lang, code }
    }

    fn parse_inline_code(&mut self) -> Node {
        self.consume_str("`");
        let mut code = String::new();
        while !self.eof() {
            if self.starts_with("`") {
                self.consume_str("`");
                break;
            }
            code.push(self.next().unwrap());
        }
        Node::InlineCode(code)
    }

    fn try_parse_link(&mut self) -> Option<Node> {
        let start_pos = self.pos;
        self.consume_str("[");

        // Parse link text
        let text_nodes = self.parse_nodes(&["]"]);
        if !self.starts_with("]") {
            self.pos = start_pos;
            return None;
        }
        self.consume_str("]");

        if !self.starts_with("(") {
            self.pos = start_pos;
            return None;
        }
        self.consume_str("(");

        // Read URL until ')'
        let mut url = String::new();
        while !self.eof() {
            if self.starts_with(")") {
                self.consume_str(")");
                return Some(Node::Link {
                    text: text_nodes,
                    url,
                });
            }
            url.push(self.next().unwrap());
        }

        self.pos = start_pos;
        None
    }

    fn has_matching_delimiter(&self, delim: &str) -> bool {
        let delim_chars: Vec<char> = delim.chars().collect();
        let start = self.pos + delim_chars.len();
        let end = self.chars.len();

        let mut i = start;
        while i < end {
            if i + delim_chars.len() <= end {
                let mut matches = true;
                for (idx, &c) in delim_chars.iter().enumerate() {
                    if self.chars[i + idx] != c {
                        matches = false;
                        break;
                    }
                }
                if matches {
                    // For code blocks and inline code, we do not require the non-whitespace check
                    if delim == "```" || delim == "`" {
                        return true;
                    }
                    // For formatting blocks, closing delimiter must be preceded by a non-whitespace char
                    if i > start {
                        let prev_char = self.chars[i - 1];
                        if !prev_char.is_whitespace() {
                            return true;
                        }
                    }
                }
            }
            i += 1;
        }
        false
    }

    fn can_open_delimiter(&self, delim: &str) -> bool {
        let delim_len = delim.chars().count();
        if self.pos + delim_len >= self.chars.len() {
            return false;
        }
        let next_char = self.chars[self.pos + delim_len];
        if next_char.is_whitespace() {
            return false;
        }
        // Single/double underscore cannot be preceded by alphanumeric to prevent inside-word matching
        if (delim == "_" || delim == "__") && self.pos > 0 {
            let prev_char = self.chars[self.pos - 1];
            if prev_char.is_alphanumeric() {
                return false;
            }
        }
        self.has_matching_delimiter(delim)
    }

    fn parse_nodes(&mut self, delimiters: &[&str]) -> Vec<Node> {
        let mut nodes = Vec::new();
        let mut current_text = String::new();

        let flush_text = |nodes: &mut Vec<Node>, current_text: &mut String| {
            if !current_text.is_empty() {
                nodes.push(Node::Text(current_text.clone()));
                current_text.clear();
            }
        };

        while !self.eof() {
            // Check if we hit any of the delimiters that would close the current group
            let mut matched_delimiter = false;
            for delim in delimiters {
                if self.starts_with(delim) {
                    matched_delimiter = true;
                    break;
                }
            }
            if matched_delimiter {
                break;
            }

            // Check for Code Block
            if self.starts_with("```") {
                if self.has_matching_delimiter("```") {
                    flush_text(&mut nodes, &mut current_text);
                    nodes.push(self.parse_code_block());
                    continue;
                } else {
                    current_text.push('`');
                    self.next();
                    continue;
                }
            }

            // Check for Inline Code
            if self.starts_with("`") {
                if self.has_matching_delimiter("`") {
                    flush_text(&mut nodes, &mut current_text);
                    nodes.push(self.parse_inline_code());
                    continue;
                } else {
                    current_text.push('`');
                    self.next();
                    continue;
                }
            }

            // Check for Link
            if self.starts_with("[") {
                flush_text(&mut nodes, &mut current_text);
                if let Some(link_node) = self.try_parse_link() {
                    nodes.push(link_node);
                } else {
                    current_text.push('[');
                    self.next();
                }
                continue;
            }

            // Check for Bold (**)
            if self.starts_with("**") {
                if self.can_open_delimiter("**") {
                    flush_text(&mut nodes, &mut current_text);
                    self.consume_str("**");
                    let inner = self.parse_nodes(&["**"]);
                    if self.starts_with("**") {
                        self.consume_str("**");
                    }
                    nodes.push(Node::Bold(inner));
                    continue;
                } else {
                    current_text.push('*');
                    self.next();
                    continue;
                }
            }

            // Check for Underline (__)
            if self.starts_with("__") {
                if self.can_open_delimiter("__") {
                    flush_text(&mut nodes, &mut current_text);
                    self.consume_str("__");
                    let inner = self.parse_nodes(&["__"]);
                    if self.starts_with("__") {
                        self.consume_str("__");
                    }
                    nodes.push(Node::Underline(inner));
                    continue;
                } else {
                    current_text.push('_');
                    self.next();
                    continue;
                }
            }

            // Check for Italic (* or _)
            if self.starts_with("*") {
                if self.can_open_delimiter("*") {
                    flush_text(&mut nodes, &mut current_text);
                    self.consume_str("*");
                    let inner = self.parse_nodes(&["*"]);
                    if self.starts_with("*") {
                        self.consume_str("*");
                    }
                    nodes.push(Node::Italic(inner));
                    continue;
                } else {
                    current_text.push('*');
                    self.next();
                    continue;
                }
            }

            if self.starts_with("_") {
                if self.can_open_delimiter("_") {
                    flush_text(&mut nodes, &mut current_text);
                    self.consume_str("_");
                    let inner = self.parse_nodes(&["_"]);
                    if self.starts_with("_") {
                        self.consume_str("_");
                    }
                    nodes.push(Node::Italic(inner));
                    continue;
                } else {
                    current_text.push('_');
                    self.next();
                    continue;
                }
            }

            // Check for Strikethrough (~~)
            if self.starts_with("~~") {
                if self.can_open_delimiter("~~") {
                    flush_text(&mut nodes, &mut current_text);
                    self.consume_str("~~");
                    let inner = self.parse_nodes(&["~~"]);
                    if self.starts_with("~~") {
                        self.consume_str("~~");
                    }
                    nodes.push(Node::Strikethrough(inner));
                    continue;
                } else {
                    current_text.push('~');
                    self.next();
                    continue;
                }
            }

            // Check for backslash escaping
            if self.starts_with("\\") {
                self.consume_str("\\");
                if let Some(next_c) = self.next() {
                    current_text.push(next_c);
                } else {
                    current_text.push('\\');
                }
                continue;
            }

            // Check for Headings at start of line or start of input
            let is_start_of_line =
                self.pos == 0 || (self.pos > 0 && self.chars[self.pos - 1] == '\n');
            if is_start_of_line && self.starts_with("#") {
                let mut hash_count = 0;
                while let Some(c) = self.peek_n(hash_count) {
                    if c == '#' {
                        hash_count += 1;
                    } else {
                        break;
                    }
                }
                if let Some(' ') = self.peek_n(hash_count) {
                    flush_text(&mut nodes, &mut current_text);
                    self.pos += hash_count + 1; // Consume '# '

                    let mut heading_text_chars = Vec::new();
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        heading_text_chars.push(self.next().unwrap());
                    }
                    let heading_text: String = heading_text_chars.into_iter().collect();

                    let mut heading_parser = Parser::new(&heading_text);
                    let heading_nodes = heading_parser.parse_nodes(&[]);
                    nodes.push(Node::Bold(heading_nodes));
                    continue;
                }
            }

            // Default
            current_text.push(self.next().unwrap());
        }

        flush_text(&mut nodes, &mut current_text);
        nodes
    }
}

fn escape_text(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
            | '{' | '}' | '.' | '!' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

fn escape_code(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            '\\' | '`' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

fn escape_url(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            '\\' | ')' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

impl Node {
    fn format_to_v2(&self) -> String {
        match self {
            Node::Text(s) => escape_text(s),
            Node::Bold(children) => {
                let inner: String = children.iter().map(|c| c.format_to_v2()).collect();
                format!("*{inner}*")
            }
            Node::Italic(children) => {
                let inner: String = children.iter().map(|c| c.format_to_v2()).collect();
                format!("_{inner}_")
            }
            Node::Underline(children) => {
                let inner: String = children.iter().map(|c| c.format_to_v2()).collect();
                format!("__{inner}__")
            }
            Node::Strikethrough(children) => {
                let inner: String = children.iter().map(|c| c.format_to_v2()).collect();
                format!("~{inner}~")
            }
            Node::CodeBlock { lang, code } => {
                let escaped_code = escape_code(code);
                format!("```{}\n{}```", lang, escaped_code)
            }
            Node::InlineCode(code) => {
                let escaped_code = escape_code(code);
                format!("`{escaped_code}`")
            }
            Node::Link { text, url } => {
                let inner: String = text.iter().map(|c| c.format_to_v2()).collect();
                let escaped_url = escape_url(url);
                format!("[{inner}]({escaped_url})")
            }
        }
    }
}

/// Convert a standard Markdown string to a Telegram-compatible MarkdownV2 string.
pub fn parse_markdown_to_v2(input: &str) -> String {
    let mut parser = Parser::new(input);
    let nodes = parser.parse_nodes(&[]);
    nodes.iter().map(|n| n.format_to_v2()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_escaping() {
        assert_eq!(parse_markdown_to_v2("hello. world!"), "hello\\. world\\!");
        assert_eq!(parse_markdown_to_v2("A -> B"), "A \\-\\> B");
    }

    #[test]
    fn test_bold_and_italic() {
        assert_eq!(parse_markdown_to_v2("**bold**"), "*bold*");
        assert_eq!(parse_markdown_to_v2("*italic*"), "_italic_");
        assert_eq!(parse_markdown_to_v2("_italic_"), "_italic_");
        assert_eq!(parse_markdown_to_v2("__underline__"), "__underline__");
    }

    #[test]
    fn test_code_blocks() {
        assert_eq!(
            parse_markdown_to_v2("```rust\nlet x = 1;\n```"),
            "```rust\nlet x = 1;\n```"
        );
        assert_eq!(parse_markdown_to_v2("`inline.code`"), "`inline.code`");
    }

    #[test]
    fn test_headings() {
        assert_eq!(parse_markdown_to_v2("# Heading 1"), "*Heading 1*");
        assert_eq!(parse_markdown_to_v2("## Heading 2"), "*Heading 2*");
    }

    #[test]
    fn test_links() {
        assert_eq!(
            parse_markdown_to_v2("[google](https://google.com?a=1&b=2)"),
            "[google](https://google.com?a=1&b=2)"
        );
    }

    #[test]
    fn test_unmatched_delimiters() {
        // Unmatched delimiters are treated literally and do not create nested formatting or corrupt text
        assert_eq!(parse_markdown_to_v2("2 * 3"), "2 \\* 3");
        assert_eq!(parse_markdown_to_v2("snake_case"), "snake\\_case");
        assert_eq!(
            parse_markdown_to_v2("unfinished `code"),
            "unfinished \\`code"
        );
    }
}
