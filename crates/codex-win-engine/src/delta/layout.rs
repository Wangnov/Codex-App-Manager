//! Combine a ZIP container's central directory with its `AppxBlockMap.xml`
//! into one per-entry view the planner can walk directly: for each payload
//! file, the exact absolute byte offset of every block, plus the small
//! leftover regions (a possible "closer tail" and a data descriptor) the
//! block map does not describe. Mirrors the feasibility prototype's
//! `layout.py` `build_layout`, which this module's tests cross-check against
//! real historical release pairs' behavior (see `delta::planner` tests).

use std::collections::HashMap;

use crate::appx_blockmap::{self, AppxBlockMap, AppxBlockMapFile};
use crate::delta::zip_format::{self, ByteSource, ZipLayout};
use crate::EngineError;

pub const APPX_BLOCK_MAP_ENTRY_NAME: &str = "AppxBlockMap.xml";
const MAX_APPX_BLOCK_MAP_XML_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ResolvedBlock {
    pub hash_base64: String,
    pub size: u64,
    /// Absolute byte offset of this block's on-disk data within the package.
    pub offset: u64,
    /// `true` for a stored (uncompressed) block (`<Block>` without `Size`):
    /// the on-disk bytes are the hashed content itself. Otherwise the block
    /// is an independent raw-deflate stream whose inflated bytes are hashed.
    pub stored: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedBlockMapFile {
    pub lfh_size: u64,
    /// `local_header_offset + lfh_size` — the start of this entry's
    /// compressed data.
    pub data_offset: u64,
    pub blocks: Vec<ResolvedBlock>,
    /// Sum of the declared block sizes.
    pub block_data_size: u64,
}

#[derive(Debug, Clone)]
pub struct ResolvedFile {
    /// ZIP entry name, exactly as stored in the central directory (may be
    /// percent-encoded — MakeAppx encodes reserved characters like `@`).
    pub name: String,
    pub local_header_offset: u64,
    /// Exclusive end of this entry's on-disk record: the next entry's local
    /// header offset (entries are physically contiguous), or the central
    /// directory offset for the last entry. Free — no extra reads needed.
    pub end_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    /// `Some` when `AppxBlockMap.xml` describes this entry's blocks —
    /// `None` for `AppxBlockMap.xml` itself and the handful of other
    /// ancillary entries (`[Content_Types].xml`, `AppxSignature.p7x`,
    /// code-integrity catalogs) that are always fetched in full.
    pub block_map_file: Option<ResolvedBlockMapFile>,
}

impl ResolvedFile {
    /// Bytes between the last described block and the end of the compressed
    /// data region. Zero for every real package observed so far, but never
    /// assumed to be — always fetched fresh rather than reused.
    pub fn closer_tail_len(&self) -> u64 {
        match &self.block_map_file {
            Some(bf) => self.compressed_size.saturating_sub(bf.block_data_size),
            None => 0,
        }
    }

    /// Trailing bytes after the compressed data up to `end_offset` — a data
    /// descriptor when the general-purpose bit 3 flag is set (0, 16 or 24
    /// bytes), otherwise 0. Always fetched fresh, never reused.
    pub fn data_descriptor_len(&self) -> u64 {
        match &self.block_map_file {
            Some(bf) => self
                .end_offset
                .saturating_sub(bf.data_offset.saturating_add(self.compressed_size)),
            None => 0,
        }
    }

    pub fn is_covered_by_block_map(&self) -> bool {
        self.block_map_file.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct PackageLayout {
    pub file_size: u64,
    pub central_directory_offset: u64,
    pub central_directory_size: u64,
    /// Sorted by `local_header_offset`.
    pub files: Vec<ResolvedFile>,
    /// Block-map `<File>` entries that had no matching ZIP central-directory
    /// entry. Never fatal on its own (the final whole-file SHA-256 check is
    /// the real safety net) but worth surfacing — it means this package's
    /// block map disagrees with its own ZIP directory.
    pub unmatched_block_map_files: Vec<String>,
}

fn percent_decode_lenient(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_val(bytes[index + 1]), hex_val(bytes[index + 2]))
            {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Combine an already-parsed [`ZipLayout`] with an already-parsed
/// [`AppxBlockMap`] into the per-entry [`PackageLayout`] the planner walks.
/// Matching follows the same rule the feasibility prototype validated on
/// real releases: `AppxBlockMap.xml`'s `File/@Name` is backslash-separated
/// and never percent-encoded, while the ZIP entry name for the same payload
/// file may be (MakeAppx percent-encodes reserved characters); the raw ZIP
/// name is tried first, the percent-decoded ZIP name second.
pub fn resolve_package_layout(
    zip: ZipLayout,
    block_map: Option<AppxBlockMap>,
) -> Result<PackageLayout, EngineError> {
    let mut by_slash_name: HashMap<String, AppxBlockMapFile> = HashMap::new();
    if let Some(block_map) = block_map {
        for file in block_map.files {
            by_slash_name.insert(file.name.replace('\\', "/"), file);
        }
    }

    // The planner turns these offsets into byte spans (`start..end`); a
    // reversed or out-of-file span would underflow its length arithmetic.
    // Reject inconsistent metadata here so the caller gets a clean `Err` and
    // falls back to the full download.
    if zip.central_directory_offset > zip.file_size {
        return Err(EngineError::Msix(format!(
            "central directory offset {} is past the end of the package ({} bytes)",
            zip.central_directory_offset, zip.file_size
        )));
    }

    let mut entries = zip.entries;
    entries.sort_by_key(|entry| entry.local_header_offset);

    let mut files = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let end_offset = entries
            .get(index + 1)
            .map(|next| next.local_header_offset)
            .unwrap_or(zip.central_directory_offset);
        if end_offset < entry.local_header_offset {
            return Err(EngineError::Msix(format!(
                "ZIP entry {:?} has a local header offset ({}) past the end of its record ({end_offset}); the central directory is inconsistent",
                entry.name, entry.local_header_offset
            )));
        }

        let matched = by_slash_name
            .remove(&entry.name)
            .or_else(|| by_slash_name.remove(&percent_decode_lenient(&entry.name)));

        let block_map_file = matched
            .map(|file| -> Result<ResolvedBlockMapFile, EngineError> {
                // Unlike `portable.rs`'s extractor (which never reads this
                // field), the delta engine cannot locate a single block
                // without it: `lfh_size` is what turns a local-header
                // offset into the start of compressed data.
                let lfh_size = file.lfh_size.ok_or_else(|| {
                    EngineError::Msix(format!(
                        "AppxBlockMap.xml File {:?} is missing LfhSize, required for delta layout",
                        file.name
                    ))
                })?;
                // Both operands are untrusted package bytes -- `lfh_size` in
                // particular is an arbitrary attacker/corruption-controlled
                // u64 straight out of the AppxBlockMap.xml text (unlike the
                // small, packer-bounded value `zip_format::local_header_size`
                // itself computes) -- so a plain `+` here could overflow
                // instead of the caller getting a clean `Err` and falling
                // back to a full download.
                let data_offset = entry.local_header_offset.checked_add(lfh_size).ok_or_else(|| {
                    EngineError::Msix(format!(
                        "AppxBlockMap.xml File {:?} has an LfhSize that overflows its local header offset",
                        file.name
                    ))
                })?;
                // Every block size is untrusted XML text too: a corrupt
                // map must yield a clean `Err` (full-download fallback), not
                // an overflow panic (debug) or a wrapped offset (release),
                // and the blocks must fit inside the entry's own on-disk
                // record.
                let mut offset = data_offset;
                let mut blocks = Vec::with_capacity(file.blocks.len());
                for block in &file.blocks {
                    blocks.push(ResolvedBlock {
                        hash_base64: block.hash_base64.clone(),
                        size: block.size,
                        offset,
                        stored: block.stored,
                    });
                    offset = offset.checked_add(block.size).ok_or_else(|| {
                        EngineError::Msix(format!(
                            "AppxBlockMap.xml File {:?} has block sizes that overflow the package offset",
                            file.name
                        ))
                    })?;
                }
                if offset > end_offset {
                    return Err(EngineError::Msix(format!(
                        "AppxBlockMap.xml File {:?} declares blocks ending at {offset}, past its ZIP entry end {end_offset}",
                        file.name
                    )));
                }
                // The central directory's own compressed size is untrusted
                // as well; the planner does offset arithmetic with it, so
                // the compressed data must end inside the entry's record.
                data_offset
                    .checked_add(entry.compressed_size)
                    .filter(|end| *end <= end_offset)
                    .ok_or_else(|| {
                        EngineError::Msix(format!(
                            "ZIP entry {:?} declares a compressed size ({}) that runs past its record end {end_offset}",
                            entry.name, entry.compressed_size
                        ))
                    })?;
                Ok(ResolvedBlockMapFile {
                    lfh_size,
                    data_offset,
                    // Equal to the sum of the declared block sizes, already
                    // overflow-checked by the loop above.
                    block_data_size: offset - data_offset,
                    blocks,
                })
            })
            .transpose()?;

        files.push(ResolvedFile {
            name: entry.name.clone(),
            local_header_offset: entry.local_header_offset,
            end_offset,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            block_map_file,
        });
    }

    let mut unmatched_block_map_files = by_slash_name.into_keys().collect::<Vec<_>>();
    unmatched_block_map_files.sort();

    Ok(PackageLayout {
        file_size: zip.file_size,
        central_directory_offset: zip.central_directory_offset,
        central_directory_size: zip.central_directory_size,
        files,
        unmatched_block_map_files,
    })
}

/// Parse `AppxBlockMap.xml`'s XML text out of an already-parsed ZIP layout,
/// reading (and, since real MSIX packages deflate-compress it, inflating)
/// just that one entry's bytes rather than the whole package. Returns
/// `Ok(None)` when the package has no such entry.
pub fn read_block_map_xml<S: ByteSource>(
    source: &S,
    zip: &ZipLayout,
) -> Result<Option<String>, EngineError> {
    let Some(entry) = zip
        .entries
        .iter()
        .find(|entry| entry.name == APPX_BLOCK_MAP_ENTRY_NAME)
    else {
        return Ok(None);
    };
    if entry.uncompressed_size > MAX_APPX_BLOCK_MAP_XML_BYTES {
        return Err(EngineError::Msix(format!(
            "AppxBlockMap.xml is unexpectedly large: {} bytes",
            entry.uncompressed_size
        )));
    }
    let bytes = zip_format::read_entry_decompressed(source, entry)?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|err| EngineError::Msix(format!("AppxBlockMap.xml is not UTF-8: {err}")))
}

/// Parse a package's full layout (ZIP central directory + resolved block
/// map) directly from a [`ByteSource`] — the one entry point most callers
/// want, whether `source` is a local base file already in memory or a
/// remote package fetched a `Range` GET at a time.
pub fn build_package_layout<S: ByteSource>(source: &S) -> Result<PackageLayout, EngineError> {
    let zip = zip_format::parse_zip_layout(source)?;
    let block_map = match read_block_map_xml(source, &zip)? {
        Some(xml) => Some(appx_blockmap::parse_appx_block_map_xml(&xml)?),
        None => None,
    };
    resolve_package_layout(zip, block_map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::zip_format::CentralDirectoryEntry;

    fn entry(name: &str, lho: u64, csize: u64, usize_: u64) -> CentralDirectoryEntry {
        CentralDirectoryEntry {
            name: name.to_string(),
            method: 8,
            flags: 0,
            crc32: 0,
            compressed_size: csize,
            uncompressed_size: usize_,
            local_header_offset: lho,
        }
    }

    fn block_map_file(name: &str, usize_: u64, lfh_size: u64, blocks: &[(&str, u64)]) -> AppxBlockMapFile {
        AppxBlockMapFile {
            name: name.to_string(),
            uncompressed_size: usize_,
            lfh_size: Some(lfh_size),
            blocks: blocks
                .iter()
                .map(|(hash, size)| appx_blockmap::AppxBlock {
                    hash_base64: hash.to_string(),
                    size: *size,
                    stored: false,
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_block_offsets_and_end_offset_from_next_entry() {
        let zip = ZipLayout {
            file_size: 10_000,
            central_directory_offset: 9_000,
            central_directory_size: 500,
            entries: vec![
                entry("app/a.bin", 0, 210, 300),
                entry("app/b.bin", 5_000, 100, 100),
            ],
        };
        let block_map = AppxBlockMap {
            files: vec![block_map_file(
                r"app\a.bin",
                300,
                50,
                &[("h1", 120), ("h2", 80)],
            )],
        };
        let layout = resolve_package_layout(zip, Some(block_map)).unwrap();
        assert_eq!(layout.files.len(), 2);
        let a = &layout.files[0];
        assert_eq!(a.end_offset, 5_000);
        let bf = a.block_map_file.as_ref().unwrap();
        assert_eq!(bf.data_offset, 50);
        assert_eq!(bf.blocks[0].offset, 50);
        assert_eq!(bf.blocks[1].offset, 50 + 120);
        // compressed_size (210) exceeds the declared block data (200): the
        // extra 10 bytes are a "closer tail" that must always be fetched
        // fresh, never assumed reusable.
        assert_eq!(a.closer_tail_len(), 10);
        let b = &layout.files[1];
        assert_eq!(b.end_offset, 9_000);
        assert!(!b.is_covered_by_block_map());
        assert!(layout.unmatched_block_map_files.is_empty());
    }

    #[test]
    fn matches_percent_encoded_zip_names_against_plain_block_map_names() {
        let zip = ZipLayout {
            file_size: 1_000,
            central_directory_offset: 900,
            central_directory_size: 50,
            entries: vec![entry("app/node_modules/%40oai/file.js", 0, 40, 40)],
        };
        let block_map = AppxBlockMap {
            files: vec![block_map_file(
                r"app\node_modules\@oai\file.js",
                40,
                40,
                &[("h", 40)],
            )],
        };
        let layout = resolve_package_layout(zip, Some(block_map)).unwrap();
        assert!(layout.files[0].is_covered_by_block_map());
        assert!(layout.unmatched_block_map_files.is_empty());
    }

    #[test]
    fn surfaces_unmatched_block_map_entries_without_failing() {
        let zip = ZipLayout {
            file_size: 1_000,
            central_directory_offset: 900,
            central_directory_size: 50,
            entries: vec![entry("app/a.bin", 0, 40, 40)],
        };
        let block_map = AppxBlockMap {
            files: vec![block_map_file(r"app\ghost.bin", 40, 40, &[("h", 40)])],
        };
        let layout = resolve_package_layout(zip, Some(block_map)).unwrap();
        assert!(!layout.files[0].is_covered_by_block_map());
        assert_eq!(layout.unmatched_block_map_files, vec!["app/ghost.bin"]);
    }

    #[test]
    fn block_sizes_that_overflow_or_overrun_the_entry_are_a_clean_error() {
        let zip = || ZipLayout {
            file_size: 10_000,
            central_directory_offset: 9_000,
            central_directory_size: 500,
            entries: vec![entry("app/a.bin", 0, 210, 300)],
        };
        // Sum of sizes overflows u64.
        let overflowing = AppxBlockMap {
            files: vec![block_map_file(
                r"app\a.bin",
                300,
                50,
                &[("h1", u64::MAX), ("h2", 2)],
            )],
        };
        let err = resolve_package_layout(zip(), Some(overflowing)).unwrap_err();
        assert!(err.to_string().contains("overflow"), "{err}");

        // Declared blocks run past the end of the ZIP entry's own record
        // (here the central directory at 9_000).
        let overrunning = AppxBlockMap {
            files: vec![block_map_file(r"app\a.bin", 300, 50, &[("h1", 9_500)])],
        };
        let err = resolve_package_layout(zip(), Some(overrunning)).unwrap_err();
        assert!(err.to_string().contains("past its ZIP entry end"), "{err}");
    }

    #[test]
    fn a_compressed_size_that_overflows_or_overruns_the_entry_is_a_clean_error() {
        let block_map = || AppxBlockMap {
            files: vec![block_map_file(r"app\a.bin", 300, 50, &[("h1", 100)])],
        };
        for bad_size in [u64::MAX - 10, 9_000] {
            let zip = ZipLayout {
                file_size: 10_000,
                central_directory_offset: 9_000,
                central_directory_size: 500,
                entries: vec![entry("app/a.bin", 0, bad_size, 300)],
            };
            let err = resolve_package_layout(zip, Some(block_map())).unwrap_err();
            assert!(err.to_string().contains("compressed size"), "{bad_size}: {err}");
        }
    }

    #[test]
    fn inconsistent_entry_and_central_directory_offsets_are_a_clean_error() {
        // Last entry's local header lies beyond the central directory.
        let zip = ZipLayout {
            file_size: 10_000,
            central_directory_offset: 9_000,
            central_directory_size: 500,
            entries: vec![entry("app/a.bin", 9_500, 10, 10)],
        };
        let err = resolve_package_layout(zip, None).unwrap_err();
        assert!(err.to_string().contains("inconsistent"), "{err}");

        // Central directory claimed past EOF.
        let zip = ZipLayout {
            file_size: 10_000,
            central_directory_offset: 10_500,
            central_directory_size: 500,
            entries: vec![entry("app/a.bin", 0, 10, 10)],
        };
        let err = resolve_package_layout(zip, None).unwrap_err();
        assert!(err.to_string().contains("past the end of the package"), "{err}");
    }
}
