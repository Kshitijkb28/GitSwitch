//! Speeds in words: "12.4 MB/s" or "99.2 Mbps" (decimal, as networks count),
//! and the four characters the Windows tray icon has room for ("12M").

/// A speed for people: bytes (B/s, KB/s, MB/s, GB/s) or bits (bps … Gbps).
pub fn full(bytes_per_s: f64, bits: bool) -> String {
    let units: [&str; 4] = if bits { ["bps", "Kbps", "Mbps", "Gbps"] } else { ["B/s", "KB/s", "MB/s", "GB/s"] };
    let mut v = if bits { bytes_per_s * 8.0 } else { bytes_per_s };
    if !v.is_finite() || v < 0.0 {
        v = 0.0;
    }
    let mut i = 0;
    // Below one kilo is shown in kilos: "0.4 KB/s" reads better than "412 B/s".
    v /= 1000.0;
    i += 1;
    while v >= 999.95 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    // Decided on the value as it will print: 99.96 is "100", not "100.0".
    let text = if (v * 10.0).round() / 10.0 < 100.0 { format!("{:.1}", v) } else { format!("{:.0}", v) };
    format!("{} {}", text, units[i])
}

/// At most four characters: "0K", "3.4K", "340K", "1.2M", "12M", "120M",
/// "1.2G". Bytes use K/M/G, bits k/m/g. (The Windows tray icon's.)
#[cfg_attr(not(windows), allow(dead_code))]
pub fn compact(bytes_per_s: f64, bits: bool) -> String {
    let letters: [char; 3] = if bits { ['k', 'm', 'g'] } else { ['K', 'M', 'G'] };
    let mut v = if bits { bytes_per_s * 8.0 } else { bytes_per_s };
    if !v.is_finite() || v < 0.0 {
        v = 0.0;
    }
    v /= 1000.0;
    let mut i = 0;
    loop {
        // What it would print as, rounded: move up a unit when it no longer fits.
        let one_decimal = v < 9.95;
        let shown = if one_decimal { (v * 10.0).round() / 10.0 } else { v.round() };
        if shown >= 1000.0 && i < letters.len() - 1 {
            v /= 1000.0;
            i += 1;
            continue;
        }
        if shown >= 1000.0 {
            // Beyond what four characters hold (a thousand gigabytes a second).
            return format!("999{}", letters[i]);
        }
        if v < 0.05 && i == 0 {
            return format!("0{}", letters[0]);
        }
        return if one_decimal { format!("{:.1}{}", v, letters[i]) } else { format!("{:.0}{}", v, letters[i]) };
    }
}

/// A fixed-width number for the menu bar, so the text doesn't jump as it
/// changes: padded with figure spaces (as wide as a digit).
pub fn padded(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n >= width {
        return text.to_string();
    }
    format!("{}{}", "\u{2007}".repeat(width - n), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_read_as_people_expect() {
        assert_eq!(full(0.0, false), "0.0 KB/s");
        assert_eq!(full(412.0, false), "0.4 KB/s");
        assert_eq!(full(12_400_000.0, false), "12.4 MB/s");
        assert_eq!(full(340_000.0, false), "340 KB/s");
        assert_eq!(full(999_960.0, false), "1.0 MB/s", "rounds up into the next unit");
        assert_eq!(full(99_960.0, false), "100 KB/s", "not 100.0");
        assert_eq!(full(99_940.0, false), "99.9 KB/s");
        assert_eq!(full(12_499_000.0, true), "100 Mbps", "bits too");
        assert_eq!(full(12_400_000.0, true), "99.2 Mbps");
        assert_eq!(full(2_500_000_000.0, true), "20.0 Gbps");
        assert_eq!(full(f64::NAN, false), "0.0 KB/s");
    }

    #[test]
    fn the_icon_form_never_needs_more_than_four_characters() {
        assert_eq!(compact(0.0, false), "0K");
        assert_eq!(compact(3_400.0, false), "3.4K");
        assert_eq!(compact(340_000.0, false), "340K");
        assert_eq!(compact(999_600.0, false), "1.0M");
        assert_eq!(compact(1_240_000.0, false), "1.2M");
        assert_eq!(compact(12_400_000.0, false), "12M");
        assert_eq!(compact(120_000_000.0, false), "120M");
        assert_eq!(compact(1_200_000_000.0, false), "1.2G");
        assert_eq!(compact(12_400_000.0, true), "99m");
        for v in [0.0, 1.0, 9_949.0, 9_951.0, 99_499.0, 999_499.0, 999_999.0, 9_999_999.0, 99_999_999.0, 999_999_999.0, 5e12] {
            assert!(compact(v, false).chars().count() <= 4, "{v} -> {}", compact(v, false));
            assert!(compact(v, true).chars().count() <= 4, "{v} bits -> {}", compact(v, true));
        }
    }

    #[test]
    fn padding_keeps_the_width() {
        assert_eq!(padded("1.2", 5).chars().count(), 5);
        assert_eq!(padded("12345", 3), "12345");
    }
}
