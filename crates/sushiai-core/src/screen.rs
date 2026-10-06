/// Scrollback lines kept per session screen.
pub const SCROLLBACK_LINES: usize = 2000;

/// Terminal screen model: feed raw output, read a formatted snapshot.
pub struct Screen {
    parser: vt100::Parser,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Screen {
            parser: vt100::Parser::new(rows, cols, SCROLLBACK_LINES),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows, cols);
    }

    /// `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// Escape sequences that redraw the current screen from a blank terminal.
    pub fn snapshot(&self) -> Vec<u8> {
        self.parser.screen().contents_formatted()
    }

    /// Visible text without formatting.
    pub fn text(&self) -> String {
        self.parser.screen().contents()
    }
}
