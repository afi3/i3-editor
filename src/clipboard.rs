//! Clipboard: the internal buffer is authoritative; OSC 52 is best effort.

use std::io::Write;

/// Terminal OSC 52 implementations truncate or ignore payloads beyond roughly
/// 64 KiB of base64; the internal clipboard stays authoritative.
pub const MAX_OSC52_ENCODED: usize = 64 * 1024;

/// Ask the terminal to place `text` in the system clipboard.
/// Terminals that do not implement OSC 52 ignore it, so the internal clipboard
/// is always written first. Returns a warning when the payload is too large to
/// send; an unsupported terminal is not an error.
pub fn copy_osc52(text: &str) -> Option<&'static str> {
    if text.is_empty() {
        return None;
    }
    // The encoded size is exact, so an oversized payload is refused before the
    // copy that encoding would perform.
    let encoded_len = text.len().div_ceil(3) * 4;
    if let Some(warning) = oversize(encoded_len) {
        return Some(warning);
    }
    let encoded = base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\u{1b}]52;c;{encoded}\u{1b}\\");
    let _ = stdout.flush();
    None
}

fn oversize(encoded_len: usize) -> Option<&'static str> {
    (encoded_len > MAX_OSC52_ENCODED).then_some("OSC 52 payload exceeds 64 KiB")
}

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).map_or(0, |byte| u32::from(*byte));
        let third = chunk.get(2).map_or(0, |byte| u32::from(*byte));
        let packed = (first << 16) | (second << 8) | third;
        out.push(char::from(TABLE[((packed >> 18) & 63) as usize]));
        out.push(char::from(TABLE[((packed >> 12) & 63) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[((packed >> 6) & 63) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[(packed & 63) as usize])
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{MAX_OSC52_ENCODED, base64, oversize};

    #[test]
    fn encodes_padding_correctly() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn oversized_payloads_are_reported() {
        assert_eq!(oversize(MAX_OSC52_ENCODED), None);
        assert_eq!(
            oversize(MAX_OSC52_ENCODED + 1),
            Some("OSC 52 payload exceeds 64 KiB")
        );
    }

    #[test]
    fn encodes_utf8_bytes() {
        assert_eq!(base64("\u{4f60}".as_bytes()), "5L2g");
    }
}
