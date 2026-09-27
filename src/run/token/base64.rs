//! URL-safe base64 without padding, for storing tokens as text. Written out
//! here rather than taken from a crate, since tokens need only these two
//! functions.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Every 3 bytes become 4 characters of 6 bits each; a shorter last group
/// becomes one character more than it has bytes.
pub(super) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = match *chunk {
            [first, second, third] => u32::from_be_bytes([0, first, second, third]),
            [first, second] => u32::from_be_bytes([0, first, second, 0]),
            [first] => u32::from_be_bytes([0, first, 0, 0]),
            _ => 0,
        };
        for i in 0..chunk.len() + 1 {
            let six_bits = (bits >> (18 - 6 * i)) & 0b11_1111;
            out.push(char::from(ALPHABET[six_bits as usize]));
        }
    }
    out
}

/// The bytes `text` holds, or `None` if it isn't base64. Correct padding and
/// the standard alphabet's `+` and `/` are read too, but nothing else, so a
/// token has one text per alphabet.
pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
    let unpadded = text.trim_end_matches('=');
    let padding = text.len() - unpadded.len();
    let unpadded = unpadded.as_bytes();
    // a last character on its own can't make a whole byte
    if unpadded.len() % 4 == 1 {
        return None;
    }
    if padding > 0 && (padding > 2 || (unpadded.len() + padding) % 4 != 0) {
        return None;
    }
    let mut alphabet = None;
    let mut out = Vec::with_capacity(unpadded.len() * 3 / 4);
    for chunk in unpadded.chunks(4) {
        let mut bits = 0u32;
        for (i, &character) in chunk.iter().enumerate() {
            let (six_bits, uses) = six_bits_of(character)?;
            if let Some(uses) = uses {
                if *alphabet.get_or_insert(uses) != uses {
                    return None;
                }
            }
            bits |= six_bits << (18 - 6 * i);
        }
        // the bits past the last whole byte must be zero, or two texts
        // would read as the same token
        let unused = 24 - 8 * (chunk.len() - 1);
        if bits & ((1 << unused) - 1) != 0 {
            return None;
        }
        let [_, first, second, third] = bits.to_be_bytes();
        out.extend_from_slice(&[first, second, third][..chunk.len() - 1]);
    }
    Some(out)
}

/// The two alphabets, which differ in their last two characters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Alphabet {
    UrlSafe,
    Standard,
}

/// A character's six bits, and the alphabet it belongs to when only one
/// has it.
fn six_bits_of(character: u8) -> Option<(u32, Option<Alphabet>)> {
    let (value, alphabet) = match character {
        b'A'..=b'Z' => (character - b'A', None),
        b'a'..=b'z' => (character - b'a' + 26, None),
        b'0'..=b'9' => (character - b'0' + 52, None),
        b'-' => (62, Some(Alphabet::UrlSafe)),
        b'_' => (63, Some(Alphabet::UrlSafe)),
        b'+' => (62, Some(Alphabet::Standard)),
        b'/' => (63, Some(Alphabet::Standard)),
        _ => return None,
    };
    Some((u32::from(value), alphabet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for len in 0..70 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = encode(&bytes);
            assert_eq!(decode(&text).unwrap(), bytes);
            let padded = format!("{text}{}", "=".repeat((4 - text.len() % 4) % 4));
            assert_eq!(decode(&padded).unwrap(), bytes);
        }
    }

    #[test]
    fn each_token_has_one_text() {
        assert_eq!(decode("AA"), Some(vec![0]));
        assert!(decode("AB").is_none(), "unused bits set");
        assert_eq!(decode("AAE"), Some(vec![0, 1]));
        assert!(decode("AAB").is_none(), "unused bits set");
        assert!(decode("AA===").is_none(), "too much padding");
        assert!(decode("AAA==").is_none(), "padding past a group");
        assert!(decode("A=A").is_none());
        assert_eq!(decode("-_-_"), decode("+/+/"));
        assert!(decode("-/-_").is_none(), "two alphabets");
        assert!(decode("a").is_none());
        assert!(decode("ab$c").is_none());
    }
}
