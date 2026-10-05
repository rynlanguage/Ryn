use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub struct SourceFile {
    path: PathBuf,
    text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    /// Reads a source file from disk and records the path for diagnostics.
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let text = fs::read_to_string(&path)?;
        Ok(Self::new(path, text))
    }

    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        let bytes = text.as_bytes();
        let mut offset = 0;
        while offset < bytes.len() {
            match bytes[offset] {
                b'\r' => {
                    offset += 1;
                    if bytes.get(offset) == Some(&b'\n') {
                        offset += 1;
                    }
                    line_starts.push(offset);
                }
                b'\n' => {
                    offset += 1;
                    line_starts.push(offset);
                }
                _ => offset += 1,
            }
        }
        Self {
            path: path.into(),
            text,
            line_starts,
        }
    }

    /// Returns the path associated with this source buffer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the one-based line and display column for a byte offset.
    ///
    /// Offsets are clamped to the source length and rounded down to a UTF-8
    /// character boundary. Tabs advance to the next four-column stop, and
    /// Unicode characters use their terminal display width.
    pub fn line_column(&self, offset: usize) -> (usize, usize) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line_index = self
            .line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line_index];
        let column = visual_width(&self.text[line_start..offset], 0) + 1;
        (line_index + 1, column)
    }
}

#[derive(Debug)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "error[{}]: {}", self.code, self.message)?;
        if let Some(help) = &self.help {
            write!(formatter, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

impl Error for Diagnostic {}

impl Diagnostic {
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

impl Diagnostic {
    pub fn render(&self, source_file: &SourceFile) -> String {
        let source = source_file.text();
        let mut start = self.span.start.min(source.len());
        while !source.is_char_boundary(start) {
            start -= 1;
        }
        let (line, column) = source_file.line_column(start);
        let line_index = line - 1;
        let line_start = source_file.line_starts[line_index];
        let line_end = source[start..]
            .find(['\n', '\r'])
            .map_or(source.len(), |offset| start + offset);
        let source_line = &source[line_start..line_end];
        let mut end = self.span.end.max(start).min(line_end);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let marker_width = visual_width(&source[start..end], column - 1).max(1);
        let source_line = expand_tabs(source_line);
        let gutter_width = line.to_string().len();
        let gutter = " ".repeat(gutter_width);
        // Tabs are expanded consistently in both the source line and caret column.
        let rendered = format!(
            "error[{}]: {}\n  --> {}:{line}:{column}\n{gutter} |\n{line:>gutter_width$} | {source_line}\n{gutter} | {}^{}",
            self.code,
            self.message,
            source_file.path.display(),
            " ".repeat(column - 1),
            "~".repeat(marker_width - 1),
        );
        if let Some(help) = &self.help {
            format!("{rendered}\n  help: {help}")
        } else {
            rendered
        }
    }
}

fn visual_width(text: &str, initial_column: usize) -> usize {
    let mut column = initial_column;
    let mut parts = text.split('\t').peekable();
    while let Some(part) = parts.next() {
        column += UnicodeWidthStr::width(part);
        if parts.peek().is_some() {
            column += 4 - column % 4;
        }
    }
    column - initial_column
}

fn expand_tabs(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut column = 0;
    let mut parts = text.split('\t').peekable();
    while let Some(part) = parts.next() {
        expanded.push_str(part);
        column += UnicodeWidthStr::width(part);
        if parts.peek().is_some() {
            let spaces = 4 - column % 4;
            expanded.push_str(&" ".repeat(spaces));
            column += spaces;
        }
    }
    expanded
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, SourceFile, Span};
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn loads_source_text_and_preserves_its_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ryn-source-{}-{unique}.ryn", std::process::id()));
        fs::write(&path, "fn main() {}").expect("source fixture is written");

        let source = SourceFile::load(&path).expect("source file loads");

        assert_eq!(source.path(), path);
        assert_eq!(source.text(), "fn main() {}");
        fs::remove_file(path).expect("temporary source is removed");
    }

    #[test]
    fn source_file_exposes_its_path_and_text() {
        let source = SourceFile::new("src/main.ryn", "fn main() {}");

        assert_eq!(source.path(), Path::new("src/main.ryn"));
        assert_eq!(source.text(), "fn main() {}");
    }

    #[test]
    fn source_file_reports_one_based_display_positions() {
        let text = "\tA猫e\u{301}x\r\nnext";
        let source = SourceFile::new("main.ryn", text);
        let x = text.find('x').expect("x is present");
        let cat = text.find('猫').expect("wide character is present");
        let next = text.find("next").expect("second line is present");

        assert_eq!(source.line_column(x), (1, 9));
        assert_eq!(source.line_column(cat + 1), (1, 6));
        assert_eq!(source.line_column(next), (2, 1));
        assert_eq!(source.line_column(usize::MAX), (2, 5));
    }

    #[test]
    fn source_file_handles_lone_cr_and_mixed_line_endings() {
        let text = "one\rtwo\r\n猫\nlast\r";
        let source = SourceFile::new("main.ryn", text);

        assert_eq!(source.line_column(text.find("two").unwrap()), (2, 1));
        assert_eq!(source.line_column(text.find('猫').unwrap()), (3, 1));
        assert_eq!(source.line_column(text.find("last").unwrap()), (4, 1));
        assert_eq!(source.line_column(text.len()), (5, 1));
    }

    #[test]
    fn renders_source_line_and_exact_highlight() {
        let source = "fn main() {\n    print(missing)\n}";
        let start = source.find("missing").expect("identifier is present");
        let diagnostic = Diagnostic {
            code: "R0203",
            message: "unknown variable `missing`".into(),
            help: None,
            span: Span {
                start,
                end: start + "missing".len(),
            },
        };

        assert_eq!(
            diagnostic.render(&SourceFile::new(Path::new("src/main.ryn"), source)),
            "error[R0203]: unknown variable `missing`\n  --> src/main.ryn:2:11\n  |\n2 |     print(missing)\n  |           ^~~~~~~"
        );
    }

    #[test]
    fn handles_empty_and_multiline_spans_without_panicking() {
        let source = "fn main() {\r\n    print(1)\r\n}";
        let diagnostic = Diagnostic {
            code: "R0012",
            message: "expected expression".into(),
            help: None,
            span: Span { start: 13, end: 24 },
        };
        let rendered = diagnostic.render(&SourceFile::new("main.ryn", source));
        assert!(rendered.contains("main.ryn:2:1"));
        assert!(rendered.contains("^"));

        let empty = Diagnostic {
            code: "R0000",
            message: "empty span".into(),
            help: None,
            span: Span {
                start: source.len(),
                end: source.len(),
            },
        };
        assert!(
            empty
                .render(&SourceFile::new("main.ryn", source))
                .ends_with('^')
        );
    }

    #[test]
    fn renders_diagnostics_on_lone_cr_lines() {
        let text = "fn main() {\r let value = missing\r}\r";
        let start = text.find("missing").unwrap();
        let diagnostic = Diagnostic {
            code: "R0203",
            message: "unknown variable `missing`".into(),
            help: None,
            span: Span {
                start,
                end: start + "missing".len(),
            },
        };

        let rendered = diagnostic.render(&SourceFile::new("legacy.ryn", text));

        assert!(rendered.contains("legacy.ryn:2:14"));
        assert!(rendered.contains("2 |  let value = missing"));
        assert!(rendered.contains(&format!("  | {}^~~~~~~", " ".repeat(13))));
    }

    #[test]
    fn aligns_caret_when_source_uses_tabs() {
        let source = "fn main() {\n\tprint(missing)\n}";
        let start = source.find("missing").expect("identifier is present");
        let diagnostic = Diagnostic {
            code: "R0203",
            message: "unknown variable `missing`".into(),
            help: None,
            span: Span {
                start,
                end: start + "missing".len(),
            },
        };
        let rendered = diagnostic.render(&SourceFile::new("main.ryn", source));
        assert!(rendered.contains("2 |     print(missing)"));
        assert!(rendered.contains("  |           ^~~~~~~"));
    }

    #[test]
    fn aligns_caret_after_wide_and_combining_unicode_characters() {
        let source = "a猫e\u{301}missing";
        let start = source.find("missing").expect("identifier is present");
        let diagnostic = Diagnostic {
            code: "R0203",
            message: "unknown variable `missing`".into(),
            help: None,
            span: Span {
                start,
                end: start + "missing".len(),
            },
        };

        assert_eq!(
            diagnostic.render(&SourceFile::new("main.ryn", source)),
            "error[R0203]: unknown variable `missing`\n  --> main.ryn:1:5\n  |\n1 | a猫e\u{301}missing\n  |     ^~~~~~~"
        );
    }

    #[test]
    fn aligns_caret_after_joined_emoji_sequences() {
        let source = "a👩‍💻missing";
        let start = source.find("missing").expect("identifier is present");
        let diagnostic = Diagnostic {
            code: "R0203",
            message: "unknown variable `missing`".into(),
            help: None,
            span: Span {
                start,
                end: start + "missing".len(),
            },
        };

        assert_eq!(
            diagnostic.render(&SourceFile::new("main.ryn", source)),
            "error[R0203]: unknown variable `missing`\n  --> main.ryn:1:4\n  |\n1 | a👩‍💻missing\n  |    ^~~~~~~"
        );
    }

    #[test]
    fn highlights_an_unexpected_emoji_at_its_full_display_width() {
        let source = "💥";
        let diagnostic = Diagnostic {
            code: "R0005",
            message: "unexpected character '💥'".into(),
            help: None,
            span: Span {
                start: 0,
                end: source.len(),
            },
        };

        assert_eq!(
            diagnostic.render(&SourceFile::new("main.ryn", source)),
            "error[R0005]: unexpected character '💥'\n  --> main.ryn:1:1\n  |\n1 | 💥\n  | ^~"
        );
    }

    #[test]
    fn renders_structured_help_after_the_source_highlight() {
        let diagnostic = Diagnostic {
            code: "R0204",
            message: "`value` is immutable".into(),
            span: Span { start: 0, end: 5 },
            help: Some("declare `value` with `let mut`".into()),
        };
        assert!(
            diagnostic
                .render(&SourceFile::new("main.ryn", "value"))
                .ends_with("  help: declare `value` with `let mut`")
        );
    }
}
