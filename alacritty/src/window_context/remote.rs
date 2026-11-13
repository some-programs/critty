//! Remote control of terminal windows through the IPC socket.

use std::collections::HashMap;
use std::fmt::Write;
use std::hash::BuildHasher;

use winit::window::WindowId;

use alacritty_terminal::event::Notify;
use alacritty_terminal::grid::{Dimensions, GridCell};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::cli::IpcReadScreen;

use super::WindowContext;

impl WindowContext {
    /// Text of the window's rows selected by `args`.
    ///
    /// Without `lines` or `all`, these are the rows currently visible. With `lines`, they are the
    /// last rows of scrollback history and screen, and with `all` every row of both; either way
    /// they end at the bottom of the screen, whatever the scroll position.
    ///
    /// Soft-wrapped lines are joined and trailing whitespace is dropped, like a selection copy.
    /// With `ansi`, colors and text attributes are included as SGR escape sequences.
    pub fn read_screen(&self, args: &IpcReadScreen) -> String {
        let terminal = self.terminal.lock();
        let grid = terminal.grid();
        let (top, bottom) = match args.lines {
            _ if args.all => (grid.topmost_line(), grid.bottommost_line()),
            Some(0) => return String::new(),
            Some(lines) => {
                let bottom = grid.bottommost_line();
                (bottom - (lines.min(grid.total_lines()) - 1), bottom)
            },
            None => {
                let top = Line(-(grid.display_offset() as i32));
                (top, top + (terminal.screen_lines() - 1))
            },
        };
        if !args.ansi {
            let start = Point::new(top, Column(0));
            let end = Point::new(bottom, terminal.last_column());
            return terminal.bounds_to_string(start, end);
        }

        let mut text = String::new();
        let mut style = Style::default();
        for line in top.0..=bottom.0 {
            let row = &terminal.grid()[Line(line)];
            let length = row[..].iter().rposition(|cell| !cell.is_empty()).map_or(0, |i| i + 1);

            for cell in &row[..][..length] {
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let cell_style = Style::of(cell);
                if cell_style != style {
                    cell_style.write_sgr(&mut text);
                    style = cell_style;
                }
                // Tabs are stored as a tab followed by spaces; print only the spaces' width.
                text.push(if cell.c == '\t' { ' ' } else { cell.c });
                text.extend(cell.zerowidth().into_iter().flatten());
            }

            if !row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE) {
                // Reset first, so a background color does not run into the next line.
                if style != Style::default() {
                    text.push_str("\x1b[0m");
                    style = Style::default();
                }
                text.push('\n');
            }
        }
        if style != Style::default() {
            text.push_str("\x1b[0m");
        }

        text.strip_suffix('\n').map(str::to_owned).unwrap_or(text)
    }

    /// Write text to the terminal's input, as if it was typed.
    pub fn send_text(&self, text: String) {
        self.notifier.notify(text.into_bytes());
    }
}

/// Window targeted by a remote control request, or an error message if it does not exist.
///
/// Without an explicit ID, the window with the lowest ID is used.
pub fn remote_window<'a, S: BuildHasher>(
    windows: &'a HashMap<WindowId, WindowContext, S>,
    window_id: Option<&WindowId>,
) -> Result<&'a WindowContext, String> {
    match window_id {
        Some(window_id) => windows
            .get(window_id)
            .ok_or_else(|| format!("no window with ID {}", u64::from(*window_id))),
        None => windows
            .iter()
            .min_by_key(|(id, _)| u64::from(**id))
            .map(|(_, window)| window)
            .ok_or_else(|| String::from("no window")),
    }
}

/// Cell attributes that are expressed as SGR parameters.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

impl Style {
    const FLAGS: Flags = Flags::BOLD
        .union(Flags::DIM)
        .union(Flags::ITALIC)
        .union(Flags::ALL_UNDERLINES)
        .union(Flags::INVERSE)
        .union(Flags::HIDDEN)
        .union(Flags::STRIKEOUT);

    fn of(cell: &Cell) -> Self {
        Self { fg: cell.fg, bg: cell.bg, flags: cell.flags & Self::FLAGS }
    }

    /// Write the SGR sequence that switches from any style to this one.
    fn write_sgr(&self, text: &mut String) {
        text.push_str("\x1b[0");
        for (flag, param) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::UNDERLINE, "4"),
            (Flags::DOUBLE_UNDERLINE, "4:2"),
            (Flags::UNDERCURL, "4:3"),
            (Flags::DOTTED_UNDERLINE, "4:4"),
            (Flags::DASHED_UNDERLINE, "4:5"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
        ] {
            if self.flags.contains(flag) {
                text.push(';');
                text.push_str(param);
            }
        }
        write_color(text, self.fg, 30, 90, 38);
        write_color(text, self.bg, 40, 100, 48);
        text.push('m');
    }
}

/// Write the SGR parameters of a color; the default foreground and background write nothing.
fn write_color(text: &mut String, color: Color, normal: usize, bright: usize, extended: usize) {
    let _ = match color {
        Color::Named(name) if (name as usize) < 8 => write!(text, ";{}", normal + name as usize),
        Color::Named(name) if (name as usize) < 16 => {
            write!(text, ";{}", bright + name as usize - 8)
        },
        Color::Named(_) => Ok(()),
        Color::Indexed(index) => write!(text, ";{extended};5;{index}"),
        Color::Spec(rgb) => write!(text, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b),
    };
}
