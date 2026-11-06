//! The Windows tray icon with the speed drawn in it: two lines of at most
//! four characters (download above, upload below), in a pixel font made for
//! the icon's size, so they stay sharp at 16, 20, 24 or 32 pixels.
//! (macOS and Linux show text beside the icon instead.)
#![cfg_attr(not(windows), allow(dead_code))]

/// A glyph: its width and its rows, the leftmost pixel in the highest bit.
struct Glyph {
    w: u32,
    rows: &'static [u8],
}

/// 3×5, for 16–23 pixels (and doubled for 32 and up).
fn small(c: char) -> Glyph {
    let rows: &'static [u8] = match c {
        '0' => &[0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => &[0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => &[0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => &[0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => &[0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => &[0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => &[0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => &[0b111, 0b001, 0b001, 0b010, 0b010],
        '8' => &[0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => &[0b111, 0b101, 0b111, 0b001, 0b111],
        '.' => return Glyph { w: 1, rows: &[0, 0, 0, 0, 1] },
        'K' | 'k' => &[0b101, 0b101, 0b110, 0b101, 0b101],
        'M' | 'm' => &[0b101, 0b111, 0b111, 0b101, 0b101],
        'G' | 'g' => &[0b111, 0b100, 0b101, 0b101, 0b111],
        _ => &[0, 0, 0, 0, 0],
    };
    Glyph { w: 3, rows }
}

/// 5×7, for 24–31 pixels.
fn large(c: char) -> Glyph {
    let rows: &'static [u8] = match c {
        '0' => &[0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => &[0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => &[0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => &[0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => &[0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => &[0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => &[0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => &[0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => &[0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => &[0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        '.' => return Glyph { w: 1, rows: &[0, 0, 0, 0, 0, 0, 1] },
        'K' | 'k' => &[0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'M' | 'm' => &[0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'G' | 'g' => &[0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        _ => &[0, 0, 0, 0, 0, 0, 0],
    };
    Glyph { w: 5, rows }
}

/// The font and how many screen pixels each of its pixels takes, for an icon size.
fn font_for(size: u32) -> (fn(char) -> Glyph, u32, u32) {
    if size >= 32 {
        (small, 5, size / 16)
    } else if size >= 24 {
        (large, 7, 1)
    } else {
        (small, 5, 1)
    }
}

/// How wide a line of text is, in screen pixels.
pub fn width(text: &str, size: u32) -> u32 {
    let (font, _, scale) = font_for(size);
    let glyphs: u32 = text.chars().map(|c| font(c).w).sum();
    (glyphs + text.chars().count().saturating_sub(1) as u32) * scale
}

/// The icon's pixels (RGBA, `size`×`size`): each line centred, in its colour,
/// on a clear background.
pub fn render(lines: &[(String, [u8; 4])], size: u32) -> Vec<u8> {
    let mut img = vec![0u8; (size * size * 4) as usize];
    let (font, gh, scale) = font_for(size);
    let line_h = gh * scale;
    let gap = scale.max(size / 16);
    let n = lines.len() as u32;
    let total = n * line_h + n.saturating_sub(1) * gap;
    let mut y0 = size.saturating_sub(total) / 2;
    for (text, colour) in lines {
        let w = width(text, size).min(size);
        let mut x = size.saturating_sub(w) / 2;
        for c in text.chars() {
            let g = font(c);
            for (row, bits) in g.rows.iter().enumerate() {
                for col in 0..g.w {
                    if bits >> (g.w - 1 - col) & 1 == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = (x + col * scale + dx, y0 + row as u32 * scale + dy);
                            if px < size && py < size {
                                let i = ((py * size + px) * 4) as usize;
                                img[i..i + 4].copy_from_slice(colour);
                            }
                        }
                    }
                }
            }
            x += (g.w + 1) * scale;
        }
        y0 += line_h + gap;
    }
    img
}

/// Download blue and upload orange, as in the tab's graph and the speed bar:
/// the palette's steps for a light or a dark taskbar.
pub fn colours(light_taskbar: bool) -> ([u8; 4], [u8; 4]) {
    if light_taskbar {
        ([0x2a, 0x78, 0xd6, 0xff], [0xeb, 0x68, 0x34, 0xff])
    } else {
        ([0x39, 0x87, 0xe5, 0xff], [0xd9, 0x59, 0x26, 0xff])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_characters_fit_every_icon_size_on_two_lines() {
        let (down, up) = colours(false);
        for size in [16u32, 20, 24, 32, 40, 48] {
            for text in ["1.2M", "340K", "999G", "0K", "88.8"] {
                assert!(width(text, size) <= size, "{text} at {size}: {}", width(text, size));
            }
            let img = render(&[("1.2M".into(), down), ("340K".into(), up)], size);
            assert_eq!(img.len(), (size * size * 4) as usize);
            let lit = img.chunks(4).filter(|p| p[3] != 0).count();
            assert!(lit > 20, "something is drawn at {size}: {lit}");
            assert!(img.chunks(4).filter(|p| p[3] != 0).all(|p| p == down || p == up), "only the two colours");
        }
        let (dl, _) = colours(true);
        assert_ne!(dl, down, "a light taskbar gets darker text");
    }
}
