//! ZIP entry names must honor the language flag, not UTF-8 autodetection.

use std::{borrow::Cow, io::Read, path::PathBuf, str::Utf8Error};

// Unicode mappings for CP437 bytes 0x80..=0xff. ASCII bytes map unchanged.
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// Decode the effective name using bit 11 (UTF-8) or CP437.
///
/// ZIP 9 applies valid UnicodePath extra fields, including their CRC and UTF-8
/// checks, to name_raw() and sets bit 11 before exposing the entry. Using these
/// effective bytes and flags preserves that override without guessing encodings.
/// Malformed names explicitly marked UTF-8 are rejected, not decoded as CP437.
pub fn entry_name<'a, R: Read>(
    entry: &'a zip::read::ZipFile<'_, R>,
) -> Result<Cow<'a, str>, Utf8Error> {
    let raw = entry.name_raw();
    if entry.flags().as_u16() & (1 << 11) != 0 || raw.is_ascii() {
        return std::str::from_utf8(raw).map(Cow::Borrowed);
    }
    Ok(Cow::Owned(
        raw.iter()
            .map(|&byte| {
                if byte < 0x80 {
                    char::from(byte)
                } else {
                    CP437_HIGH[usize::from(byte - 0x80)]
                }
            })
            .collect(),
    ))
}

/// Validate the decoded name as a relative path on both Windows and Unix.
/// Normalize separators and internal `.`/`..` without allowing root escape.
/// Callers must use this returned path for subsequent checks and extraction.
pub fn enclosed_path(name: &str) -> Option<PathBuf> {
    if name.starts_with(['/', '\\']) || name.contains(['\0', ':']) {
        return None;
    }
    let mut path = PathBuf::new();
    for component in name.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." if !path.pop() => return None,
            ".." => {}
            part => path.push(part),
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(test)]
#[path = "../../test-support/zip.rs"]
mod zip_test;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_test::{archive, unicode_path_extra, Entry};
    use std::io::Cursor;

    #[test]
    fn language_flag_disambiguates_the_same_raw_name() {
        for (flags, expected) in [(0, "├⌐.webp"), (1 << 11, "é.webp")] {
            let mut entry = Entry::new(b"\xc3\xa9.webp", b"data");
            entry.flags = flags;
            let mut zip = zip::ZipArchive::new(Cursor::new(archive(&[entry]))).unwrap();
            let file = zip.by_index(0).unwrap();
            assert_eq!(file.flags().as_u16(), flags);
            assert_eq!(entry_name(&file).unwrap(), expected);
        }
    }

    #[test]
    fn unicode_path_requires_valid_crc_and_utf8() {
        let raw = b"\xc3\xa9.webp";
        for (unicode, valid_crc, expected) in [
            ("资源.webp".as_bytes(), true, "资源.webp"),
            ("资源.webp".as_bytes(), false, "├⌐.webp"),
            (b"\xff.webp".as_slice(), true, "├⌐.webp"),
        ] {
            let extra = unicode_path_extra(raw, unicode, valid_crc);
            let mut entry = Entry::new(raw, b"data");
            entry.extra = &extra;
            let mut zip = zip::ZipArchive::new(Cursor::new(archive(&[entry]))).unwrap();
            assert_eq!(entry_name(&zip.by_index(0).unwrap()).unwrap(), expected);
        }
    }

    #[test]
    fn invalid_utf8_is_only_allowed_without_the_utf8_flag() {
        for flags in [0, 1 << 11] {
            let mut entry = Entry::new(b"caf\x82.bin", b"data");
            entry.flags = flags;
            let mut zip = zip::ZipArchive::new(Cursor::new(archive(&[entry]))).unwrap();
            let file = zip.by_index(0).unwrap();
            if flags == 0 {
                assert_eq!(entry_name(&file).unwrap(), "café.bin");
            } else {
                assert!(entry_name(&file).is_err());
            }
        }
    }

    #[test]
    fn decoded_paths_cannot_escape_on_either_platform() {
        for name in [
            "",
            ".",
            "..",
            "../evil",
            "a/../../evil",
            r"a\..\..\evil",
            "/absolute",
            r"\absolute",
            r"C:\evil",
            "C:evil",
            r"\\server\share",
            "a\0b",
        ] {
            assert!(enclosed_path(name).is_none(), "{name:?}");
        }
        for name in ["assets/├⌐.webp", r"assets\├⌐.webp", "assets/x/../├⌐.webp"] {
            assert_eq!(
                enclosed_path(name).unwrap(),
                PathBuf::from("assets").join("├⌐.webp")
            );
        }
    }
}
