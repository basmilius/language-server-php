/// How a level of indentation is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(usize),
    Tab,
}

impl Indent {
    pub fn unit(self) -> String {
        match self {
            Indent::Spaces(width) => " ".repeat(width.max(1)),
            Indent::Tab => "\t".to_string(),
        }
    }

    /// The columns a level takes, for measuring a line.
    pub fn width(self) -> usize {
        match self {
            Indent::Spaces(width) => width.max(1),
            Indent::Tab => 4,
        }
    }
}

/// Where the opening brace of a declaration goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BraceStyle {
    NextLine,
    SameLine,
}

/// What the formatter may be asked. The defaults are PER Coding Style 2.0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    pub indent: Indent,
    /// The brace of a class, interface, trait or enum.
    pub class_brace: BraceStyle,
    /// The brace of a named function or a method. A closure's brace is always on its line.
    pub function_brace: BraceStyle,
    /// The blank lines between a method and its neighbors.
    pub blank_lines_between_members: usize,
    /// Pad `=` of assignments on consecutive lines to one column.
    pub align_assignments: bool,
    /// Pad `=>` of array items on consecutive lines to one column.
    pub align_array_arrows: bool,
    /// A line longer than this has its argument or parameter list broken up. Zero never wraps.
    pub line_length: usize,
}

impl Default for FormatOptions {
    fn default() -> FormatOptions {
        FormatOptions {
            indent: Indent::Spaces(4),
            class_brace: BraceStyle::NextLine,
            function_brace: BraceStyle::NextLine,
            blank_lines_between_members: 1,
            align_assignments: false,
            align_array_arrows: false,
            line_length: 120,
        }
    }
}
