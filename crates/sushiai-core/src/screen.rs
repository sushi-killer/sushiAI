/// Scrollback lines kept per session screen.
pub const SCROLLBACK_LINES: usize = 2000;

/// Terminal screen model: feed raw output, read a formatted snapshot.
pub struct Screen {
    parser: vt100::Parser,
    /// An incomplete trailing escape sequence or UTF-8 character, kept for the next `feed`
    /// so the parser rests in its ground state between feeds.
    held: Vec<u8>,
}

/// The most bytes `feed` keeps back; a longer unfinished sequence is passed on as it is.
const MAX_HELD: usize = 64 * 1024;

/// Where an unfinished escape sequence (ESC, CSI, OSC, DCS and the other string types) or a
/// partial UTF-8 character at the end of `data` starts; `data.len()` when it ends cleanly.
fn incomplete_tail(data: &[u8]) -> usize {
    if let Some(esc) = data.iter().rposition(|b| *b == 0x1b) {
        let body = &data[esc + 1..];
        let open = match body.first() {
            None => true,
            // CSI: parameters and intermediates, then a final byte.
            Some(b'[') => !body[1..].iter().any(|b| (0x40..=0x7e).contains(b)),
            // OSC ends at BEL or ST; the string types at ST.
            Some(b']') => !(body.contains(&0x07) || body.windows(2).any(|w| w == b"\x1b\\")),
            Some(b'P' | b'X' | b'^' | b'_') => !body.windows(2).any(|w| w == b"\x1b\\"),
            // Intermediates (ESC ( B, ESC # 8), then a final byte.
            Some(_) => body.iter().all(|b| (0x20..=0x2f).contains(b)),
        };
        if open {
            return esc;
        }
    }
    // A UTF-8 lead byte near the end whose continuation bytes are not all there yet.
    for back in 1..=data.len().min(3) {
        let at = data.len() - back;
        let need = match data[at] {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            0x80..=0xbf => continue,
            _ => break,
        };
        return if back < need { at } else { data.len() };
    }
    data.len()
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Screen {
            parser: vt100::Parser::new(rows, cols, SCROLLBACK_LINES),
            held: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let split = incomplete_tail(&data);
        if data.len() - split <= MAX_HELD {
            self.held = data.split_off(split);
        }
        self.parser.process(&data);
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

    /// Up to `n` of the newest lines that scrolled off the top, oldest first, each formatted
    /// (escape sequences, no line break).
    pub fn history_formatted(&mut self, n: usize) -> Vec<Vec<u8>> {
        self.history(n, |screen, cols| screen.rows_formatted(0, cols).collect())
    }

    /// Like `history_formatted`, as plain text. Wrapped rows are joined the way `text` joins
    /// them, and the last line ends with a line break unless it wraps into the live screen,
    /// so `history_text(n) + text()` reads as one text.
    pub fn history_text(&mut self, n: usize) -> String {
        let rows = self.history(n, |screen, cols| {
            screen
                .rows(0, cols)
                .enumerate()
                .map(|(i, row)| (row, screen.row_wrapped(i as u16)))
                .collect()
        });
        let mut text = String::new();
        for (row, wrapped) in rows {
            text.push_str(&row);
            if !wrapped {
                text.push('\n');
            }
        }
        text
    }

    /// vt100 0.15 panics when the scrollback offset exceeds the screen height, so only the
    /// newest screenful is reachable as it is. The screen is therefore grown (blank rows are
    /// appended at the bottom), the history is read, and the screen is shrunk again. Resizing
    /// clamps a pending-wrap cursor (written in the last column, not yet wrapped) into the
    /// last column; that state is put back by redrawing its row.
    fn history<T>(&mut self, n: usize, rows_of: impl Fn(&vt100::Screen, u16) -> Vec<T>) -> Vec<T> {
        // Clamped to the lines that exist; the view is put back before anything reads it.
        self.parser.set_scrollback(usize::MAX);
        let count = n.min(self.parser.screen().scrollback());
        self.parser.set_scrollback(0);
        if count == 0 {
            return Vec::new();
        }
        let (rows, cols) = self.parser.screen().size();
        let (cursor_row, cursor_col) = self.parser.screen().cursor_position();
        let pen = self.parser.screen().attributes_formatted();
        let grown = u16::try_from(usize::from(rows) + count).unwrap_or(u16::MAX);
        let count = usize::from(grown - rows);
        self.parser.set_size(grown, cols);
        self.parser.set_scrollback(count);
        let lines = rows_of(self.parser.screen(), cols)
            .into_iter()
            .take(count)
            .collect();
        self.parser.set_scrollback(0);
        self.parser.set_size(rows, cols);
        if cursor_col >= cols {
            // The cursor is now in the last column of its row. Draw that cell again in place
            // (a wide character from its first half): the cursor ends up waiting to wrap.
            let screen = self.parser.screen();
            let wide = screen
                .cell(cursor_row, cols - 1)
                .is_some_and(vt100::Cell::is_wide_continuation);
            let width = if wide { 2 } else { 1 };
            let cell = screen
                .rows_formatted(cols - width, width)
                .nth(usize::from(cursor_row))
                .unwrap_or_default();
            let mut redraw = Vec::new();
            if wide {
                redraw.extend(b"\x1b[D");
            }
            redraw.extend(b"\x1b[m");
            redraw.extend(cell);
            redraw.extend(b"\x1b[m");
            redraw.extend(pen);
            self.parser.process(&redraw);
        }
        lines
    }
}
