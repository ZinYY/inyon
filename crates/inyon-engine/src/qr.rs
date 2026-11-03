//! The pairing code as a QR code in the terminal, drawn the way the Bun engine drew it (uqr's
//! renderUnicodeCompact): two modules per character with the half blocks, light modules as full
//! blocks (a terminal is usually dark), and a quiet zone of two modules.

use qrcode::{Color, EcLevel, QrCode};

const BORDER: usize = 2;

/// `text` as lines of half-block characters.
pub fn render(text: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).map_err(|e| format!("Couldn't make the QR code ({e})."))?;
    let width = code.width();
    let modules = code.to_colors();
    Ok(draw(width, |x, y| modules[y * width + x] == Color::Dark))
}

/// Draws a `width` by `width` code whose module at (x, y) is dark when `dark` says so. Below the
/// last row counts as dark, which is what the engine did on an odd number of rows.
pub fn draw(width: usize, dark: impl Fn(usize, usize) -> bool) -> String {
    let size = width + 2 * BORDER;
    let at = |x: usize, y: usize| -> bool {
        if y >= size {
            return true;
        }
        if x < BORDER || y < BORDER || x >= size - BORDER || y >= size - BORDER {
            return false;
        }
        dark(x - BORDER, y - BORDER)
    };
    let mut lines = Vec::with_capacity(size.div_ceil(2));
    for row in (0..size).step_by(2) {
        let line: String = (0..size)
            .map(|col| match (at(col, row), at(col, row + 1)) {
                (false, false) => '█',
                (false, true) => '▀',
                (true, false) => '▄',
                (true, true) => ' ',
            })
            .collect();
        lines.push(line);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_is_drawn_with_half_blocks_and_a_quiet_zone() {
        let out = render("inyon://pair?m=k7m2p9x4&s=abcdefghijklmnopqrstuvwxyz0123&n=laptop").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let size = lines[0].chars().count();
        // a version 1 code is 21 modules; whatever the version, the size is odd with the border
        assert!(size >= 25 && size % 2 == 1, "{size}");
        assert_eq!(lines.len(), size.div_ceil(2));
        assert!(lines.iter().all(|l| l.chars().count() == size));
        assert!(out.chars().all(|c| matches!(c, '█' | '▀' | '▄' | ' ' | '\n')));
        // the first two rows are the quiet zone: light on both halves
        assert!(lines[0].chars().all(|c| c == '█'));
        // the last row is the quiet zone over the edge of the picture
        assert!(lines.last().unwrap().chars().all(|c| c == '▀'));
        // the finder pattern's corner: dark on both module rows, right inside the border
        assert_eq!(lines[1].chars().nth(2), Some(' '));
        assert_eq!(lines[1].chars().nth(1), Some('█'));
    }

    #[test]
    fn a_tiny_picture_shows_each_pairing() {
        // a 2x2 picture, dark on the diagonal: the border around it is light, the two module
        // rows share one line
        assert_eq!(draw(2, |x, y| x == y), "██████\n██▄▀██\n██████");
        // an odd size: the one module shares a line with the border under it, and the last
        // line pairs the bottom border with the dark below the picture
        assert_eq!(draw(1, |_, _| true), "█████\n██▄██\n▀▀▀▀▀");
    }
}
