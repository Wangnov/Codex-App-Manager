//! Stored ZIP fixtures with explicit filename bytes, flags and extra fields.
//! Do not use ZipWriter here: it sets the UTF-8 flag from the name's contents.

pub struct Entry<'a> {
    pub name: &'a [u8],
    pub data: &'a [u8],
    pub flags: u16,
    pub extra: &'a [u8],
    pub unix_mode: u32,
}

impl<'a> Entry<'a> {
    pub fn new(name: &'a [u8], data: &'a [u8]) -> Self {
        Self {
            name,
            data,
            flags: 0,
            extra: &[],
            unix_mode: 0o100644,
        }
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub fn unicode_path_extra(raw_name: &[u8], unicode_name: &[u8], valid_crc: bool) -> Vec<u8> {
    let mut extra = Vec::new();
    extra.extend(0x7075_u16.to_le_bytes());
    extra.extend(u16::try_from(5 + unicode_name.len()).unwrap().to_le_bytes());
    extra.push(1);
    extra.extend((crc32(raw_name) ^ u32::from(!valid_crc)).to_le_bytes());
    extra.extend(unicode_name);
    extra
}

pub fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let offset = u32::try_from(bytes.len()).unwrap();
        let size = u32::try_from(entry.data.len()).unwrap();
        let name_len = u16::try_from(entry.name.len()).unwrap();
        let extra_len = u16::try_from(entry.extra.len()).unwrap();
        let crc = crc32(entry.data);
        bytes.extend(b"PK\x03\x04");
        for value in [20_u16, entry.flags, 0, 0, 0] {
            bytes.extend(value.to_le_bytes());
        }
        for value in [crc, size, size] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(name_len.to_le_bytes());
        bytes.extend(extra_len.to_le_bytes());
        bytes.extend(entry.name);
        bytes.extend(entry.extra);
        bytes.extend(entry.data);

        central.extend(b"PK\x01\x02");
        for value in [(3 << 8) | 20_u16, 20, entry.flags, 0, 0, 0] {
            central.extend(value.to_le_bytes());
        }
        for value in [crc, size, size] {
            central.extend(value.to_le_bytes());
        }
        for value in [name_len, extra_len, 0, 0, 0] {
            central.extend(value.to_le_bytes());
        }
        central.extend((entry.unix_mode << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(entry.name);
        central.extend(entry.extra);
    }
    let central_offset = u32::try_from(bytes.len()).unwrap();
    let central_len = u32::try_from(central.len()).unwrap();
    bytes.extend(central);
    bytes.extend(b"PK\x05\x06");
    let count = u16::try_from(entries.len()).unwrap();
    for value in [0_u16, 0, count, count] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(central_len.to_le_bytes());
    bytes.extend(central_offset.to_le_bytes());
    bytes.extend(0_u16.to_le_bytes());
    bytes
}

#[test]
fn fixture_crc_matches_the_standard_check_value() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}
