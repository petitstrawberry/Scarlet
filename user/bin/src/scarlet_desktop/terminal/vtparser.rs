//! VT100/ANSI escape sequence parser for Scarlet Terminal
//!
//! This module parses ANSI escape sequences and renders them to the terminal buffer.

/// ANSI escape sequence parser state
#[derive(Debug, Clone, Copy, PartialEq)]
enum ParserState {
    Normal,
    Escape,
    CSI,
    DCS,
    OSC,
}

/// VT100/ANSI escape sequence parser
pub struct VtParser {
    state: ParserState,
    params: scarlet_std::vec::Vec<u16>,
    current_param: u16,
    osc_data: scarlet_std::vec::Vec<u8>,
}

impl VtParser {
    pub fn new() -> Self {
        Self {
            state: ParserState::Normal,
            params: scarlet_std::vec::Vec::new(),
            current_param: 0,
            osc_data: scarlet_std::vec::Vec::new(),
        }
    }

    /// Parse a single character and return the action to take
    pub fn parse(&mut self, c: char) -> VtAction {
        match self.state {
            ParserState::Normal => {
                if c == '\x1b' {
                    self.state = ParserState::Escape;
                    VtAction::None
                } else if c == '\r' {
                    VtAction::CarriageReturn
                } else if c == '\n' {
                    VtAction::LineFeed
                } else if c == '\t' {
                    VtAction::Tab
                } else if c == '\x08' {
                    VtAction::Backspace
                } else if c == '\x07' {
                    // Bell - ignore for now
                    VtAction::None
                } else if c.is_control() {
                    VtAction::None
                } else {
                    VtAction::Print(c)
                }
            }
            ParserState::Escape => {
                match c {
                    '[' => {
                        self.state = ParserState::CSI;
                        self.params.clear();
                        self.current_param = 0;
                        VtAction::None
                    }
                    ']' => {
                        self.state = ParserState::OSC;
                        self.osc_data.clear();
                        VtAction::None
                    }
                    'P' => {
                        self.state = ParserState::DCS;
                        VtAction::None
                    }
                    'M' => {
                        // Reverse line feed (not commonly used)
                        self.state = ParserState::Normal;
                        VtAction::None
                    }
                    '=' => {
                        // Application keypad mode - ignore
                        self.state = ParserState::Normal;
                        VtAction::None
                    }
                    '>' => {
                        // Normal keypad mode - ignore
                        self.state = ParserState::Normal;
                        VtAction::None
                    }
                    _ => {
                        self.state = ParserState::Normal;
                        VtAction::None
                    }
                }
            }
            ParserState::CSI => {
                if c.is_ascii_digit() {
                    // Build parameter
                    self.current_param = self.current_param * 10 + (c as u16 - '0' as u16);
                    VtAction::None
                } else if c == ';' {
                    // Parameter separator
                    self.params.push(self.current_param);
                    self.current_param = 0;
                    VtAction::None
                } else if c.is_ascii_alphabetic() || c == '@' || c == '`' {
                    // CSI sequence terminator
                    self.params.push(self.current_param);
                    let action = self.handle_csi(c);
                    self.state = ParserState::Normal;
                    self.params.clear();
                    self.current_param = 0;
                    action
                } else {
                    self.state = ParserState::Normal;
                    VtAction::None
                }
            }
            ParserState::OSC => {
                if c == '\x07' || (c == '\x1b' && self.osc_data.last() == Some(&b'\\')) {
                    // OSC terminator (BEL or ESC \)
                    if c == '\x1b' {
                        let _ = self.osc_data.pop();
                    }
                    self.state = ParserState::Normal;
                    VtAction::None
                } else {
                    self.osc_data.push(c as u8);
                    VtAction::None
                }
            }
            ParserState::DCS => {
                if c == '\x1b' && self.osc_data.last() == Some(&b'\\') {
                    self.state = ParserState::Normal;
                    VtAction::None
                } else {
                    // Ignore DCS for now
                    self.state = ParserState::Normal;
                    VtAction::None
                }
            }
        }
    }

    fn handle_csi(&mut self, c: char) -> VtAction {
        let default = || 0;
        let p0 = self.params.get(0).map_or(0, |&x| x);
        let p1 = self.params.get(1).map_or(0, |&x| x);

        match c {
            // Cursor movement
            'A' => VtAction::CursorUp(if p0 > 0 { p0 } else { 1 }),
            'B' => VtAction::CursorDown(if p0 > 0 { p0 } else { 1 }),
            'C' => VtAction::CursorRight(if p0 > 0 { p0 } else { 1 }),
            'D' => VtAction::CursorLeft(if p0 > 0 { p0 } else { 1 }),
            'E' => VtAction::CursorNextLine(if p0 > 0 { p0 } else { 1 }),
            'F' => VtAction::CursorPreviousLine(if p0 > 0 { p0 } else { 1 }),
            'G' | '`' => VtAction::CursorHorizontalAbsolute(if p0 > 0 { p0 } else { 1 }),
            'H' | 'f' => {
                let row = if p0 > 0 { p0 } else { 1 };
                let col = if p1 > 0 { p1 } else { 1 };
                VtAction::CursorPosition(row, col)
            }
            'I' => VtAction::CursorForwardTab(if p0 > 0 { p0 } else { 1 }),
            'Z' => VtAction::CursorBackwardTab(if p0 > 0 { p0 } else { 1 }),

            // Erase functions
            'J' => VtAction::EraseDisplay(p0),
            'K' => VtAction::EraseLine(p0),
            'X' => VtAction::EraseChars(if p0 > 0 { p0 } else { 1 }),

            // Screen functions
            '@' => VtAction::InsertChars(if p0 > 0 { p0 } else { 1 }),
            'P' => VtAction::DeleteChars(if p0 > 0 { p0 } else { 1 }),
            'L' => VtAction::InsertLines(if p0 > 0 { p0 } else { 1 }),
            'M' => VtAction::DeleteLines(if p0 > 0 { p0 } else { 1 }),

            // Scrolling
            'r' => VtAction::SetScrollingRegion(p0, p1),
            'S' => VtAction::ScrollUp(if p0 > 0 { p0 } else { 1 }),
            'T' => VtAction::ScrollDown(if p0 > 0 { p0 } else { 1 }),

            // Text attributes
            'm' => VtAction::SetAttributes(self.params.clone()),

            // Mode settings
            'h' => VtAction::SetMode(p0, true),
            'l' => VtAction::SetMode(p0, false),

            // Device functions
            'c' => VtAction::DeviceAttributes,
            'n' => VtAction::DeviceStatus(p0),

            // Cursor save/restore
            's' => VtAction::SaveCursor,
            'u' => VtAction::RestoreCursor,

            _ => VtAction::None,
        }
    }

    /// Parse a buffer of data and return all actions
    pub fn parse_buffer(&mut self, data: &[u8]) -> scarlet_std::vec::Vec<VtAction> {
        let mut actions = scarlet_std::vec::Vec::new();
        for &byte in data {
            let c = byte as char;
            let action = self.parse(c);
            if action != VtAction::None {
                actions.push(action);
            }
        }
        actions
    }
}

/// Actions that can be produced by the VT parser
#[derive(Debug, Clone, PartialEq)]
pub enum VtAction {
    None,
    Print(char),
    CarriageReturn,
    LineFeed,
    Tab,
    Backspace,

    // Cursor movement
    CursorUp(u16),
    CursorDown(u16),
    CursorRight(u16),
    CursorLeft(u16),
    CursorNextLine(u16),
    CursorPreviousLine(u16),
    CursorHorizontalAbsolute(u16),
    CursorPosition(u16, u16),
    CursorForwardTab(u16),
    CursorBackwardTab(u16),

    // Erase functions
    EraseLine(u16),    // 0=to end, 1=from start, 2=all
    EraseDisplay(u16), // 0=to end, 1=from start, 2=all
    EraseChars(u16),

    // Screen functions
    InsertChars(u16),
    DeleteChars(u16),
    InsertLines(u16),
    DeleteLines(u16),

    // Scrolling
    SetScrollingRegion(u16, u16),
    ScrollUp(u16),
    ScrollDown(u16),

    // Text attributes
    SetAttribute(u16),
    SetAttributes(scarlet_std::vec::Vec<u16>),
    ResetAttributes,

    // Mode settings
    SetMode(u16, bool),

    // Device functions
    DeviceAttributes,
    DeviceStatus(u16),

    // Cursor save/restore
    SaveCursor,
    RestoreCursor,
}

impl Default for VtParser {
    fn default() -> Self {
        Self::new()
    }
}
