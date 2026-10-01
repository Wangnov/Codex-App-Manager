//! Minimal ZIP / ZIP64 central-directory and End-Of-Central-Directory
//! parsing, independent of the `zip` crate.
//!
//! The full `zip` crate (already a dependency, used by `portable.rs` to
//! *extract* an MSIX locally) is deliberately not reused here: the delta
//! planner needs the raw local-header offset and on-disk compressed size of
//! every entry so it can compute exact byte ranges to Range-fetch or copy —
//! `zip::ZipArchive` does not expose that layout, only decompressing reads.
//! This module reads just the two structures a delta plan needs (the EOCD /
//! ZIP64 EOCD and the central directory) from a byte range the caller
//! supplies, so the same code parses a local base file (already fully in
//! memory) and a remote package (fetched a `Range` GET at a time).

use crate::EngineError;

/// A byte-addressable ZIP container the parser can pull ranges from, without
/// caring whether those bytes are already in memory (a local base file) or
/// need a network round trip (a remote package, fetched via HTTP Range).
pub trait ByteSource {
    /// Total size of the underlying file in bytes.
    fn len(&self) -> u64;
    /// True when the underlying file is empty (`len() == 0`).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Read exactly `len` bytes starting at `start`. `start + len` must not
    /// exceed `len()`.
    fn read_range(&self, start: u64, len: u64) -> Result<Vec<u8>, EngineError>;
}

/// A [`ByteSource`] backed by bytes already fully resident in memory (the
/// local base MSIX).
pub struct InMemorySource<'a> {
    data: &'a [u8],
}

impl<'a> InMemorySource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl ByteSource for InMemorySource<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_range(&self, start: u64, len: u64) -> Result<Vec<u8>, EngineError> {
        let start = usize::try_from(start)
            .map_err(|_| EngineError::Msix("zip range start overflows usize".to_string()))?;
        let len = usize::try_from(len)
            .map_err(|_| EngineError::Msix("zip range length overflows usize".to_string()))?;
        // `start` and `len` individually fit `usize` here, but their *sum*
        // can still overflow it (both are derived from ZIP/AppxBlockMap
        // offsets and sizes, which are untrusted package bytes on a corrupt
        // base or remote layout) -- a plain `start + len` would then panic
        // in a debug build instead of falling through to the ordinary
        // out-of-bounds `Err` below.
        let end = start
            .checked_add(len)
            .ok_or_else(|| EngineError::Msix("zip range end overflows usize".to_string()))?;
        self.data
            .get(start..end)
            .map(|slice| slice.to_vec())
            .ok_or_else(|| {
                EngineError::Msix(format!(
                    "zip range out of bounds: start={start} len={len} total={}",
                    self.data.len()
                ))
            })
    }
}

#[derive(Debug, Clone)]
pub struct CentralDirectoryEntry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub local_header_offset: u64,
}

#[derive(Debug, Clone)]
pub struct ZipLayout {
    pub file_size: u64,
    pub central_directory_offset: u64,
    pub central_directory_size: u64,
    pub entries: Vec<CentralDirectoryEntry>,
}

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_EOCD_LOCATOR_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const ZIP64_EOCD_RECORD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0201_4b50;
const ZIP64_EXTRA_FIELD_ID: u16 = 0x0001;
/// Cap on the compressed size of an ancillary entry read in full through
/// [`read_entry_decompressed`] (in practice only `AppxBlockMap.xml`, which is
/// a few MB at most). Matches the 64 MiB uncompressed cap in `delta::layout`.
const MAX_ANCILLARY_ENTRY_COMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

/// Starting size of the tail fetched/scanned for the EOCD record. Real MSIX
/// central directories observed in the feasibility study run well under this
/// (a few hundred KiB for ~1,500 entries); doubled up to `file_size` if the
/// EOCD or its ZIP64 locator/record are not found within it.
const INITIAL_TAIL_BYTES: u64 = 1024 * 1024;
const MAX_TAIL_WIDEN_ATTEMPTS: u32 = 6;

fn u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn u64_le(bytes: &[u8], offset: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(buf)
}

/// Find the last occurrence of `needle` in `haystack` (a backward scan — the
/// EOCD record is always searched for from the end of the file, since a ZIP
/// archive comment of attacker-chosen bytes could otherwise forge an earlier
/// false-positive signature).
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).rev().find(|&start| &haystack[start..start + needle.len()] == needle)
}

/// Find the rightmost occurrence of the classic EOCD signature in `tail`
/// whose fixed 22-byte record plus its declared comment length lands
/// *exactly* at the end of `tail` (`tail`'s end is always the true EOF --
/// see [`parse_zip_layout`]). A plain rightmost-signature search is not
/// enough: a ZIP archive comment is caller-chosen bytes and can itself
/// contain `PK\x05\x06`, which would otherwise be mistaken for the real
/// record. On a false match this keeps searching strictly before it for an
/// earlier occurrence that does satisfy the length check.
fn find_valid_eocd(tail: &[u8]) -> Option<usize> {
    let mut search_end = tail.len();
    loop {
        let candidate = rfind(&tail[..search_end], &EOCD_SIGNATURE)?;
        if tail.len() - candidate >= 22 {
            let comment_len = u16_le(tail, candidate + 20) as usize;
            if candidate + 22 + comment_len == tail.len() {
                return Some(candidate);
            }
        }
        if candidate == 0 {
            return None;
        }
        search_end = candidate;
    }
}

struct EocdInfo {
    central_directory_offset: u64,
    central_directory_size: u64,
    entry_count: u64,
}

/// Parse the EOCD (and, if present, the ZIP64 EOCD locator + record) out of
/// `tail`, a byte range covering the last `tail.len()` bytes of the archive
/// (absolute offset `tail_base`). Returns an error (rather than panicking)
/// when the tail is too short to contain what it needs — the caller widens
/// the tail and retries.
fn parse_eocd(tail: &[u8], tail_base: u64) -> Result<EocdInfo, EngineError> {
    let eocd_pos = find_valid_eocd(tail)
        .ok_or_else(|| EngineError::Msix("ZIP EOCD record not found in tail".to_string()))?;
    let mut entry_count = u16_le(tail, eocd_pos + 10) as u64;
    let mut central_directory_size = u32_le(tail, eocd_pos + 12) as u64;
    let mut central_directory_offset = u32_le(tail, eocd_pos + 16) as u64;

    // The ZIP64 EOCD Locator is a fixed 20-byte record whose position is
    // defined by the spec, not discovered by search: it sits immediately
    // before the (classic) EOCD record this function just found. Checking
    // only that fixed position -- rather than searching backward for the
    // 4-byte signature anywhere earlier in `tail` -- avoids mistaking an
    // unrelated occurrence of those same four bytes inside an ordinary
    // (non-ZIP64) archive's payload or central directory for a locator that
    // isn't actually there, which would otherwise misread bogus ZIP64
    // fields or reject an entirely valid classic ZIP.
    let locator_pos = eocd_pos
        .checked_sub(20)
        .filter(|&pos| tail[pos..pos + 4] == ZIP64_EOCD_LOCATOR_SIGNATURE);
    if let Some(locator_pos) = locator_pos {
        let zip64_eocd_offset = u64_le(tail, locator_pos + 8);
        if zip64_eocd_offset < tail_base {
            return Err(EngineError::Msix(format!(
                "ZIP64 EOCD record at absolute offset {zip64_eocd_offset} lies before the fetched tail (starts at {tail_base})"
            )));
        }
        let relative = usize::try_from(zip64_eocd_offset - tail_base)
            .map_err(|_| EngineError::Msix("ZIP64 EOCD offset overflows usize".to_string()))?;
        // `relative + 56` on a `relative` derived from attacker/corruption-
        // controlled package bytes could otherwise overflow `usize` before
        // this ever gets to compare against `tail.len()` -- a debug-build
        // panic (this crate's tests, and any debug build of the app) rather
        // than the intended clean `Err` that lets the caller fall back to a
        // full download. Compare via a checked add instead of computing the
        // end offset unchecked.
        let record_end = match relative.checked_add(56) {
            Some(end) if end <= tail.len() => end,
            _ => {
                return Err(EngineError::Msix(
                    "ZIP64 EOCD record is truncated in the fetched tail".to_string(),
                ))
            }
        };
        let record = &tail[relative..record_end];
        if record[0..4] != ZIP64_EOCD_RECORD_SIGNATURE {
            return Err(EngineError::Msix(
                "ZIP64 EOCD locator points at a bad signature".to_string(),
            ));
        }
        entry_count = u64_le(record, 32);
        central_directory_size = u64_le(record, 40);
        central_directory_offset = u64_le(record, 48);
    }

    Ok(EocdInfo {
        central_directory_offset,
        central_directory_size,
        entry_count,
    })
}

/// Parse the ZIP64 extra field (header id `0x0001`) out of a central
/// directory entry's extra-field bytes, overriding any of `usize`/`csize`/
/// `lho` that the fixed-size record marked as `0xFFFFFFFF`. Field order is
/// fixed by the spec: uncompressed size, then compressed size, then local
/// header offset, then disk-start — each present only if its 32-bit
/// counterpart overflowed.
fn apply_zip64_extra(
    extra: &[u8],
    usize_overflowed: bool,
    csize_overflowed: bool,
    lho_overflowed: bool,
    uncompressed_size: &mut u64,
    compressed_size: &mut u64,
    local_header_offset: &mut u64,
) {
    let mut pos = 0usize;
    while pos + 4 <= extra.len() {
        let header_id = u16_le(extra, pos);
        let data_len = u16_le(extra, pos + 2) as usize;
        let data_start = pos + 4;
        if data_start + data_len > extra.len() {
            break;
        }
        if header_id == ZIP64_EXTRA_FIELD_ID {
            let data = &extra[data_start..data_start + data_len];
            let mut cursor = 0usize;
            if usize_overflowed && cursor + 8 <= data.len() {
                *uncompressed_size = u64_le(data, cursor);
                cursor += 8;
            }
            if csize_overflowed && cursor + 8 <= data.len() {
                *compressed_size = u64_le(data, cursor);
                cursor += 8;
            }
            if lho_overflowed && cursor + 8 <= data.len() {
                *local_header_offset = u64_le(data, cursor);
            }
        }
        pos = data_start + data_len;
    }
}

fn parse_central_directory(
    cd: &[u8],
    entry_count: u64,
) -> Result<Vec<CentralDirectoryEntry>, EngineError> {
    let mut entries = Vec::with_capacity(entry_count.min(1 << 20) as usize);
    let mut pos = 0usize;
    for _ in 0..entry_count {
        if pos + 46 > cd.len() {
            return Err(EngineError::Msix(format!(
                "central directory truncated after {} of {entry_count} entries",
                entries.len()
            )));
        }
        let signature = u32_le(cd, pos);
        if signature != CENTRAL_DIRECTORY_SIGNATURE {
            return Err(EngineError::Msix(format!(
                "bad central directory file header signature at entry {}: {signature:#010x}",
                entries.len()
            )));
        }
        let flags = u16_le(cd, pos + 8);
        let method = u16_le(cd, pos + 10);
        let crc32 = u32_le(cd, pos + 16);
        let mut compressed_size = u32_le(cd, pos + 20) as u64;
        let mut uncompressed_size = u32_le(cd, pos + 24) as u64;
        let name_len = u16_le(cd, pos + 28) as usize;
        let extra_len = u16_le(cd, pos + 30) as usize;
        let comment_len = u16_le(cd, pos + 32) as usize;
        let mut local_header_offset = u32_le(cd, pos + 42) as u64;

        let name_start = pos + 46;
        let extra_start = name_start + name_len;
        let comment_start = extra_start + extra_len;
        let entry_end = comment_start + comment_len;
        if entry_end > cd.len() {
            return Err(EngineError::Msix(format!(
                "central directory entry {} overruns the directory",
                entries.len()
            )));
        }
        let name = String::from_utf8_lossy(&cd[name_start..extra_start]).into_owned();
        let extra = &cd[extra_start..comment_start];

        let usize_overflowed = uncompressed_size == u32::MAX as u64;
        let csize_overflowed = compressed_size == u32::MAX as u64;
        let lho_overflowed = local_header_offset == u32::MAX as u64;
        if usize_overflowed || csize_overflowed || lho_overflowed {
            apply_zip64_extra(
                extra,
                usize_overflowed,
                csize_overflowed,
                lho_overflowed,
                &mut uncompressed_size,
                &mut compressed_size,
                &mut local_header_offset,
            );
        }

        entries.push(CentralDirectoryEntry {
            name,
            method,
            flags,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });
        pos = entry_end;
    }
    Ok(entries)
}

/// Parse a whole ZIP container's layout (central directory + EOCD/ZIP64
/// EOCD) from `source`, widening the tail scanned for the EOCD record up to
/// the full file size if the initial guess was too small.
pub fn parse_zip_layout<S: ByteSource>(source: &S) -> Result<ZipLayout, EngineError> {
    let total_size = source.len();
    if total_size < 22 {
        return Err(EngineError::Msix(format!(
            "file is too small to be a ZIP archive: {total_size} bytes"
        )));
    }

    let mut tail_len = total_size.min(INITIAL_TAIL_BYTES);
    let mut attempts = 0u32;
    let eocd = loop {
        let tail_base = total_size - tail_len;
        let tail = source.read_range(tail_base, tail_len)?;
        match parse_eocd(&tail, tail_base) {
            Ok(info) => break info,
            Err(err) => {
                attempts += 1;
                if tail_len >= total_size || attempts >= MAX_TAIL_WIDEN_ATTEMPTS {
                    return Err(EngineError::Msix(format!(
                        "could not locate ZIP end-of-central-directory record after widening the tail to {tail_len} bytes: {err}"
                    )));
                }
                tail_len = (tail_len * 4).min(total_size);
            }
        }
    };

    // Same overflow hazard as the ZIP64 record bounds check above: both
    // operands come straight from package bytes (local or remote,
    // untrusted either way), so a corrupt EOCD declaring values near
    // `u64::MAX` must not be able to wrap this addition into a
    // false-negative bounds check -- or, in a debug build, panic instead of
    // returning the `Err` that lets the caller fall back to a full download.
    let central_directory_end = eocd
        .central_directory_offset
        .checked_add(eocd.central_directory_size);
    if central_directory_end.is_none_or(|end| end > total_size) {
        return Err(EngineError::Msix(format!(
            "central directory (offset={} size={}) extends past end of file ({total_size} bytes)",
            eocd.central_directory_offset, eocd.central_directory_size
        )));
    }

    // A direct read for exactly the central directory's bytes. It may
    // overlap the tail already scanned above for a remote source (a second
    // small Range GET) but that is metadata cost, never counted against the
    // delta's payload savings — see `PlanStats` in `planner.rs` — and it
    // keeps this function correct regardless of how the EOCD search widened
    // its tail.
    let cd_bytes = source.read_range(eocd.central_directory_offset, eocd.central_directory_size)?;
    let entries = parse_central_directory(&cd_bytes, eocd.entry_count)?;

    Ok(ZipLayout {
        file_size: total_size,
        central_directory_offset: eocd.central_directory_offset,
        central_directory_size: eocd.central_directory_size,
        entries,
    })
}

/// Read one ZIP entry's local file header to get its exact size (fixed
/// 30-byte header + file name + extra field). Every payload file described
/// by `AppxBlockMap.xml` already carries this as `LfhSize`, so this is only
/// needed for the handful of ancillary entries the block map does not cover
/// (`AppxBlockMap.xml` itself, `[Content_Types].xml`, `AppxSignature.p7x`,
/// code-integrity catalogs) — one small extra read each, not per block.
pub fn local_header_size<S: ByteSource>(
    source: &S,
    entry: &CentralDirectoryEntry,
) -> Result<u64, EngineError> {
    let header_end = entry.local_header_offset.checked_add(30).ok_or_else(|| {
        EngineError::Msix(format!(
            "ZIP entry {:?} has a local header offset that overflows",
            entry.name
        ))
    })?;
    if header_end > source.len() {
        return Err(EngineError::Msix(format!(
            "ZIP entry {:?} local header at {} is past the end of the package ({} bytes)",
            entry.name,
            entry.local_header_offset,
            source.len()
        )));
    }
    let header = source.read_range(entry.local_header_offset, 30)?;
    let signature = u32_le(&header, 0);
    if signature != 0x0403_4b50 {
        return Err(EngineError::Msix(format!(
            "bad local file header signature for {:?}: {signature:#010x}",
            entry.name
        )));
    }
    let name_len = u16_le(&header, 26) as u64;
    let extra_len = u16_le(&header, 28) as u64;
    Ok(30 + name_len + extra_len)
}

/// Read and, if needed, decompress one ZIP entry's full content. Used only
/// to let the planner *read* an ancillary entry (chiefly `AppxBlockMap.xml`,
/// which real MSIX packages store deflate-compressed) — the delta plan
/// never writes decompressed bytes into the reconstructed package, it always
/// copies or fetches the original compressed bytes verbatim.
pub fn read_entry_decompressed<S: ByteSource>(
    source: &S,
    entry: &CentralDirectoryEntry,
) -> Result<Vec<u8>, EngineError> {
    // Every operand is untrusted package metadata: bound the read before
    // issuing it (a hostile central directory must yield an `Err`, not an
    // overflow panic, a nonsense range request, or a huge download just to
    // read a small ancillary entry).
    if entry.compressed_size > MAX_ANCILLARY_ENTRY_COMPRESSED_BYTES {
        return Err(EngineError::Msix(format!(
            "ZIP entry {:?} has an unexpectedly large compressed size ({} bytes)",
            entry.name, entry.compressed_size
        )));
    }
    let lfh_size = local_header_size(source, entry)?;
    let data_start = entry.local_header_offset.checked_add(lfh_size).ok_or_else(|| {
        EngineError::Msix(format!(
            "ZIP entry {:?} has a local header offset that overflows",
            entry.name
        ))
    })?;
    let data_end = data_start.checked_add(entry.compressed_size).ok_or_else(|| {
        EngineError::Msix(format!(
            "ZIP entry {:?} has a compressed size that overflows its data offset",
            entry.name
        ))
    })?;
    if data_end > source.len() {
        return Err(EngineError::Msix(format!(
            "ZIP entry {:?} data ends at {data_end}, past the end of the package ({} bytes)",
            entry.name,
            source.len()
        )));
    }
    let raw = source.read_range(data_start, entry.compressed_size)?;
    match entry.method {
        0 => Ok(raw),
        8 => inflate_raw_deflate(&raw, entry.uncompressed_size),
        other => Err(EngineError::Msix(format!(
            "unsupported ZIP compression method {other} for {:?}",
            entry.name
        ))),
    }
}

/// Raw-deflate inflate (no zlib/gzip header), tolerant of an isolated stream
/// that never emits its own end-of-stream marker: success is defined as
/// "produced exactly the uncompressed size the central directory declared",
/// matching how the feasibility prototype validated individual 64 KiB
/// blocks (see `reconstruction_details` in the delta feasibility report).
fn inflate_raw_deflate(data: &[u8], expected_len: u64) -> Result<Vec<u8>, EngineError> {
    let expected = usize::try_from(expected_len)
        .map_err(|_| EngineError::Msix("decompressed length overflows usize".to_string()))?;
    let mut decompress = flate2::Decompress::new(false);
    let mut out = vec![0u8; expected];
    let mut input_pos = 0usize;
    let mut output_pos = 0usize;
    loop {
        if output_pos >= expected {
            break;
        }
        if input_pos >= data.len() {
            return Err(EngineError::Msix(format!(
                "inflate ran out of input after producing {output_pos} of {expected} expected bytes"
            )));
        }
        let before_in = decompress.total_in();
        let before_out = decompress.total_out();
        let status = decompress
            .decompress(
                &data[input_pos..],
                &mut out[output_pos..],
                flate2::FlushDecompress::Sync,
            )
            .map_err(|err| EngineError::Msix(format!("inflate failed: {err}")))?;
        input_pos += (decompress.total_in() - before_in) as usize;
        output_pos += (decompress.total_out() - before_out) as usize;
        if status == flate2::Status::StreamEnd {
            break;
        }
    }
    out.truncate(output_pos);
    if out.len() != expected {
        return Err(EngineError::Msix(format!(
            "inflate produced {} bytes, expected {expected}",
            out.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }
    fn le32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// Build a minimal, valid ZIP with `stored` (uncompressed) entries so
    /// tests do not depend on any compressor's exact bytes.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        let mut local_offsets = Vec::new();

        for (name, data) in entries {
            local_offsets.push(out.len() as u32);
            out.extend_from_slice(&le32(0x0403_4b50));
            out.extend_from_slice(&le16(20)); // version needed
            out.extend_from_slice(&le16(0)); // flags
            out.extend_from_slice(&le16(0)); // method: stored
            out.extend_from_slice(&le16(0)); // mod time
            out.extend_from_slice(&le16(0)); // mod date
            let crc = crc32_stub(data);
            out.extend_from_slice(&le32(crc));
            out.extend_from_slice(&le32(data.len() as u32)); // csize
            out.extend_from_slice(&le32(data.len() as u32)); // usize
            out.extend_from_slice(&le16(name.len() as u16));
            out.extend_from_slice(&le16(0)); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
        }

        let cd_offset = out.len() as u32;
        for ((name, data), &lho) in entries.iter().zip(local_offsets.iter()) {
            cd.extend_from_slice(&le32(CENTRAL_DIRECTORY_SIGNATURE));
            cd.extend_from_slice(&le16(20)); // version made by
            cd.extend_from_slice(&le16(20)); // version needed
            cd.extend_from_slice(&le16(0)); // flags
            cd.extend_from_slice(&le16(0)); // method
            cd.extend_from_slice(&le16(0)); // mod time
            cd.extend_from_slice(&le16(0)); // mod date
            let crc = crc32_stub(data);
            cd.extend_from_slice(&le32(crc));
            cd.extend_from_slice(&le32(data.len() as u32));
            cd.extend_from_slice(&le32(data.len() as u32));
            cd.extend_from_slice(&le16(name.len() as u16));
            cd.extend_from_slice(&le16(0)); // extra len
            cd.extend_from_slice(&le16(0)); // comment len
            cd.extend_from_slice(&le16(0)); // disk start
            cd.extend_from_slice(&le16(0)); // internal attrs
            cd.extend_from_slice(&le32(0)); // external attrs
            cd.extend_from_slice(&le32(lho));
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_size = cd.len() as u32;
        out.extend_from_slice(&cd);

        out.extend_from_slice(&EOCD_SIGNATURE);
        out.extend_from_slice(&le16(0)); // disk number
        out.extend_from_slice(&le16(0)); // disk with cd
        out.extend_from_slice(&le16(entries.len() as u16)); // entries this disk
        out.extend_from_slice(&le16(entries.len() as u16)); // total entries
        out.extend_from_slice(&le32(cd_size));
        out.extend_from_slice(&le32(cd_offset));
        out.extend_from_slice(&le16(0)); // comment len

        out
    }

    /// Not a real CRC32 — tests never check it, they only exercise layout
    /// parsing, and a stub keeps this file dependency-free.
    fn crc32_stub(data: &[u8]) -> u32 {
        data.iter().fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32))
    }

    #[test]
    fn parses_simple_zip_layout() {
        let data = build_zip(&[("a.txt", b"hello"), ("dir/b.txt", b"world!!")]);
        let source = InMemorySource::new(&data);
        let layout = parse_zip_layout(&source).unwrap();
        assert_eq!(layout.entries.len(), 2);
        assert_eq!(layout.entries[0].name, "a.txt");
        assert_eq!(layout.entries[0].compressed_size, 5);
        assert_eq!(layout.entries[1].name, "dir/b.txt");
        assert_eq!(layout.entries[1].compressed_size, 7);
        assert_eq!(layout.file_size, data.len() as u64);
    }

    #[test]
    fn rejects_truncated_archive() {
        let data = build_zip(&[("a.txt", b"hi")]);
        let truncated = &data[..data.len() - 4];
        let source = InMemorySource::new(truncated);
        assert!(parse_zip_layout(&source).is_err());
    }

    /// A ZIP archive comment is caller-chosen bytes appended after the real
    /// EOCD record; here it happens to contain the EOCD signature itself. A
    /// naive rightmost-signature search would mistake that embedded bytes
    /// for the real record and either fail to parse or read bogus offsets.
    #[test]
    fn finds_the_real_eocd_past_a_comment_containing_a_fake_signature() {
        let mut data = build_zip(&[("a.txt", b"hello"), ("dir/b.txt", b"world!!")]);
        // The real EOCD record ends where `data` currently ends (comment
        // length 0). Append a comment containing an embedded, byte-for-byte
        // fake EOCD signature followed by 18 arbitrary bytes -- deliberately
        // NOT a well-formed record, so a parser that trusts it outright
        // would either misread the directory offsets or error out instead
        // of falling back to the real record.
        let mut comment = Vec::new();
        comment.extend_from_slice(&EOCD_SIGNATURE);
        comment.extend_from_slice(&[0xAA; 18]);
        let comment_len = comment.len() as u16;
        data.extend_from_slice(&comment);
        // Patch the real EOCD's comment-length field (the last 2 bytes
        // before the comment we just appended) to declare it.
        let eocd_at = data.len() - comment.len() - 22;
        data[eocd_at + 20..eocd_at + 22].copy_from_slice(&le16(comment_len));

        let source = InMemorySource::new(&data);
        let layout = parse_zip_layout(&source).unwrap();
        assert_eq!(layout.entries.len(), 2);
        assert_eq!(layout.entries[0].name, "a.txt");
        assert_eq!(layout.entries[1].name, "dir/b.txt");
    }

    /// Build a ZIP64 archive: one entry whose central-directory record marks
    /// usize/csize/lho as 0xFFFFFFFF and carries the real 64-bit values in a
    /// ZIP64 extra field, plus a ZIP64 EOCD locator + record ahead of the
    /// classic EOCD (mirroring what real MSIX packagers do for very large
    /// packages, and what this parser must handle even for small test data).
    fn build_zip64(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        let mut local_offsets = Vec::new();

        for (name, data) in entries {
            local_offsets.push(out.len() as u64);
            out.extend_from_slice(&le32(0x0403_4b50));
            out.extend_from_slice(&le16(45));
            out.extend_from_slice(&le16(0));
            out.extend_from_slice(&le16(0));
            out.extend_from_slice(&le16(0));
            out.extend_from_slice(&le16(0));
            out.extend_from_slice(&le32(crc32_stub(data)));
            out.extend_from_slice(&le32(data.len() as u32));
            out.extend_from_slice(&le32(data.len() as u32));
            out.extend_from_slice(&le16(name.len() as u16));
            out.extend_from_slice(&le16(0));
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
        }

        let cd_offset = out.len() as u64;
        for ((name, data), &lho) in entries.iter().zip(local_offsets.iter()) {
            // ZIP64 extra field: usize(8) + csize(8) + lho(8) = 24 bytes of data.
            let mut extra = Vec::new();
            extra.extend_from_slice(&le16(ZIP64_EXTRA_FIELD_ID));
            extra.extend_from_slice(&le16(24));
            extra.extend_from_slice(&(data.len() as u64).to_le_bytes());
            extra.extend_from_slice(&(data.len() as u64).to_le_bytes());
            extra.extend_from_slice(&lho.to_le_bytes());

            cd.extend_from_slice(&le32(CENTRAL_DIRECTORY_SIGNATURE));
            cd.extend_from_slice(&le16(45));
            cd.extend_from_slice(&le16(45));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le32(crc32_stub(data)));
            cd.extend_from_slice(&le32(u32::MAX)); // csize -> zip64
            cd.extend_from_slice(&le32(u32::MAX)); // usize -> zip64
            cd.extend_from_slice(&le16(name.len() as u16));
            cd.extend_from_slice(&le16(extra.len() as u16));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le16(0));
            cd.extend_from_slice(&le32(0));
            cd.extend_from_slice(&le32(u32::MAX)); // lho -> zip64
            cd.extend_from_slice(name.as_bytes());
            cd.extend_from_slice(&extra);
        }
        let cd_size = cd.len() as u64;
        out.extend_from_slice(&cd);

        // ZIP64 EOCD record.
        let zip64_eocd_offset = out.len() as u64;
        out.extend_from_slice(&ZIP64_EOCD_RECORD_SIGNATURE);
        out.extend_from_slice(&44u64.to_le_bytes()); // size of remaining record
        out.extend_from_slice(&le16(45));
        out.extend_from_slice(&le16(45));
        out.extend_from_slice(&le32(0));
        out.extend_from_slice(&le32(0));
        out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());

        // ZIP64 EOCD locator.
        out.extend_from_slice(&ZIP64_EOCD_LOCATOR_SIGNATURE);
        out.extend_from_slice(&le32(0));
        out.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
        out.extend_from_slice(&le32(1));

        // Classic EOCD (fields saturated to indicate "see ZIP64").
        out.extend_from_slice(&EOCD_SIGNATURE);
        out.extend_from_slice(&le16(0xFFFF));
        out.extend_from_slice(&le16(0xFFFF));
        out.extend_from_slice(&le16(0xFFFF));
        out.extend_from_slice(&le16(0xFFFF));
        out.extend_from_slice(&le32(u32::MAX));
        out.extend_from_slice(&le32(u32::MAX));
        out.extend_from_slice(&le16(0));

        out
    }

    /// A single deflate-compressed entry, mirroring how real MSIX packages
    /// store `AppxBlockMap.xml` (method 8) rather than stored (method 0).
    fn build_zip_with_deflate_entry(name: &str, content: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(content).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut out = Vec::new();
        out.extend_from_slice(&le32(0x0403_4b50));
        out.extend_from_slice(&le16(20));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(8)); // method: deflate
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le32(crc32_stub(content)));
        out.extend_from_slice(&le32(compressed.len() as u32));
        out.extend_from_slice(&le32(content.len() as u32));
        out.extend_from_slice(&le16(name.len() as u16));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&compressed);

        let cd_offset = out.len() as u32;
        let mut cd = Vec::new();
        cd.extend_from_slice(&le32(CENTRAL_DIRECTORY_SIGNATURE));
        cd.extend_from_slice(&le16(20));
        cd.extend_from_slice(&le16(20));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le16(8));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le32(crc32_stub(content)));
        cd.extend_from_slice(&le32(compressed.len() as u32));
        cd.extend_from_slice(&le32(content.len() as u32));
        cd.extend_from_slice(&le16(name.len() as u16));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le16(0));
        cd.extend_from_slice(&le32(0));
        cd.extend_from_slice(&le32(0));
        cd.extend_from_slice(name.as_bytes());
        let cd_size = cd.len() as u32;
        out.extend_from_slice(&cd);

        out.extend_from_slice(&EOCD_SIGNATURE);
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(1));
        out.extend_from_slice(&le16(1));
        out.extend_from_slice(&le32(cd_size));
        out.extend_from_slice(&le32(cd_offset));
        out.extend_from_slice(&le16(0));
        out
    }

    #[test]
    fn reads_and_inflates_a_deflate_compressed_entry() {
        let content = b"AppxBlockMap.xml content repeated ".repeat(200);
        let data = build_zip_with_deflate_entry("AppxBlockMap.xml", &content);
        let source = InMemorySource::new(&data);
        let layout = parse_zip_layout(&source).unwrap();
        assert_eq!(layout.entries.len(), 1);
        assert_eq!(layout.entries[0].method, 8);
        let decoded = read_entry_decompressed(&source, &layout.entries[0]).unwrap();
        assert_eq!(decoded, content);
    }

    /// The central directory is untrusted: an entry with a local header
    /// offset near `u64::MAX`, or a compressed size that is absurd or runs
    /// past the package, must be a clean `Err` (never an overflow panic, a
    /// nonsense range request, or a giant read just to fetch a small entry).
    #[test]
    fn read_entry_decompressed_rejects_hostile_offsets_and_sizes() {
        let content = b"AppxBlockMap.xml content repeated ".repeat(200);
        let data = build_zip_with_deflate_entry("AppxBlockMap.xml", &content);
        let source = InMemorySource::new(&data);
        let good = parse_zip_layout(&source).unwrap().entries[0].clone();

        let hostile = [
            CentralDirectoryEntry { local_header_offset: u64::MAX, ..good.clone() },
            CentralDirectoryEntry { local_header_offset: u64::MAX - 10, ..good.clone() },
            CentralDirectoryEntry { local_header_offset: data.len() as u64, ..good.clone() },
            CentralDirectoryEntry { compressed_size: u64::MAX, ..good.clone() },
            CentralDirectoryEntry { compressed_size: 1 << 40, ..good.clone() },
            // Within the size cap but running past the end of the package.
            CentralDirectoryEntry { compressed_size: data.len() as u64 + 1, ..good.clone() },
        ];
        for entry in &hostile {
            assert!(
                read_entry_decompressed(&source, entry).is_err(),
                "should reject offset={} csize={}",
                entry.local_header_offset,
                entry.compressed_size
            );
        }
        assert!(read_entry_decompressed(&source, &good).is_ok());
    }

    #[test]
    fn parses_zip64_layout_via_extra_field_and_eocd_record() {
        let data = build_zip64(&[("big/one.bin", b"zip64-payload-bytes"), ("two.bin", b"more")]);
        let source = InMemorySource::new(&data);
        let layout = parse_zip_layout(&source).unwrap();
        assert_eq!(layout.entries.len(), 2);
        assert_eq!(layout.entries[0].name, "big/one.bin");
        assert_eq!(
            layout.entries[0].compressed_size,
            b"zip64-payload-bytes".len() as u64
        );
        assert!(layout.entries[1].local_header_offset > 0);
    }

    /// A ZIP64 EOCD locator's offset field is 8 bytes read straight out of
    /// untrusted package bytes (local base file or remote metadata alike).
    /// A corrupt package declaring a value near `u64::MAX` must fail
    /// cleanly with `Err` -- never panic on an unchecked `relative + 56` --
    /// so `execute_delta`'s caller still gets the fall-back-to-full-download
    /// signal instead of the whole process aborting.
    #[test]
    fn a_zip64_locator_offset_near_u64_max_fails_closed_instead_of_overflowing() {
        let mut data = build_zip64(&[("big/one.bin", b"zip64-payload-bytes"), ("two.bin", b"more")]);
        let locator_pos = data
            .windows(ZIP64_EOCD_LOCATOR_SIGNATURE.len())
            .rposition(|window| window == ZIP64_EOCD_LOCATOR_SIGNATURE)
            .expect("build_zip64 always writes a ZIP64 EOCD locator");
        // Bytes [locator_pos+8 .. locator_pos+16] are the locator's 8-byte
        // little-endian "offset of the ZIP64 EOCD record" field (after the
        // 4-byte signature + 4-byte disk-number fields).
        data[locator_pos + 8..locator_pos + 16].copy_from_slice(&(u64::MAX - 10).to_le_bytes());
        let source = InMemorySource::new(&data);
        let err = parse_zip_layout(&source).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("zip64")
                || err.to_string().to_lowercase().contains("overflow"),
            "unexpected error: {err}"
        );
    }
}
