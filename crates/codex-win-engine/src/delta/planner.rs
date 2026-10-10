//! Pure planner: turn a base package's block index plus a new package's
//! resolved layout into a copy-or-fetch plan, with gap coalescing. No I/O —
//! callers hand this already-parsed [`PackageLayout`]s (see
//! `delta::layout`) so the plan itself can be unit-tested without a network
//! or a real MSIX on disk.
//!
//! Mirrors the feasibility prototype's `compare_pair.py` (`gather_ranges` +
//! `coalesce`) and `reconstruct.py` (`build_reuse_offset_index`), which the
//! delta feasibility report's measurements (8.4%–55.1% savings across 8
//! real consecutive release pairs, byte-identical reconstruction proven on
//! one pair) were produced from.

use std::collections::HashMap;

use crate::delta::layout::PackageLayout;

/// A span to copy verbatim from the local base file into the assembled
/// output — no network request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyStep {
    pub base_offset: u64,
    pub new_offset: u64,
    pub len: u64,
}

/// A coalesced span to Range-fetch from the new package's URL and write into
/// the assembled output at the same offset (the new package's own bytes are
/// the source of truth, so fetch offset == output offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchStep {
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PlannerConfig {
    /// Two fetch spans whose gap is at most this many bytes are merged into
    /// one Range GET. 256 KiB, the feasibility report's recommended default,
    /// balances request count against re-fetching already-reusable bytes
    /// that happen to sit between two changed regions.
    pub coalesce_gap: u64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            coalesce_gap: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeltaPlan {
    pub new_size: u64,
    pub copies: Vec<CopyStep>,
    /// Already coalesced per [`PlannerConfig::coalesce_gap`].
    pub fetches: Vec<FetchStep>,
    pub total_blocks: usize,
    pub reused_blocks: usize,
    /// Sum of reused block sizes before coalescing may re-fetch some of them
    /// anyway (when a reused block sits inside a coalesced gap). The plan's
    /// real savings is `new_size - fetch_bytes()`, not this figure.
    pub reused_bytes: u64,
}

impl DeltaPlan {
    pub fn fetch_bytes(&self) -> u64 {
        self.fetches.iter().map(|step| step.len).sum()
    }

    pub fn request_count(&self) -> usize {
        self.fetches.len()
    }

    /// Percentage of `new_size` avoided, after coalescing. Matches the
    /// feasibility report's `savings_pct`.
    pub fn savings_pct(&self) -> f64 {
        if self.new_size == 0 {
            return 0.0;
        }
        100.0 * (1.0 - self.fetch_bytes() as f64 / self.new_size as f64)
    }

    /// A plan not worth acting on: the client should fall back to a full
    /// download rather than pay for delta-planning network round trips that
    /// barely save anything (see the feasibility report's caveat (a): the
    /// worst observed real pair saved only 8.38%).
    pub fn worth_using(&self, min_savings_pct: f64) -> bool {
        !self.fetches.is_empty() && self.savings_pct() >= min_savings_pct
    }
}

/// Key under which a block's on-disk bytes are interchangeable between two
/// packages: `(block hash, block on-disk size, stored)`. The hash covers the
/// *uncompressed* content, so the size alone distinguishes most re-encodings,
/// but a stored block and a deflated block of identical content can (rarely,
/// for incompressible data) have the same size while holding different bytes:
/// hence the storage mode is part of the key too.
pub type ReuseKey = (String, u64, bool);

/// `(block hash, block on-disk size, stored) -> absolute offset in the base file`,
/// built once from the base package's own resolved layout. When the same
/// (hash, size) pair appears more than once in the base (content duplicated
/// across files), the first occurrence wins — matching the feasibility
/// prototype and sufficient since either location holds byte-identical data.
pub fn build_reuse_index(base: &PackageLayout) -> HashMap<ReuseKey, u64> {
    let mut index = HashMap::new();
    for file in &base.files {
        let Some(block_map_file) = &file.block_map_file else {
            continue;
        };
        for block in &block_map_file.blocks {
            index
                .entry((block.hash_base64.clone(), block.size, block.stored))
                .or_insert(block.offset);
        }
    }
    index
}

/// Build the copy-or-fetch plan for reconstructing `new_layout` given a base
/// package's block index. Every byte of the new package that is not a
/// verbatim copy from the base file ends up in exactly one (coalesced)
/// [`FetchStep`], so `copies` and `fetches` together cover every byte from
/// offset 0 to `new_layout.file_size`.
pub fn plan_delta(
    base_reuse_index: &HashMap<ReuseKey, u64>,
    new_layout: &PackageLayout,
    config: &PlannerConfig,
) -> DeltaPlan {
    let mut copies = Vec::new();
    let mut raw_fetch_spans: Vec<(u64, u64)> = Vec::new();
    let mut total_blocks = 0usize;
    let mut reused_blocks = 0usize;
    let mut reused_bytes = 0u64;

    // Bytes before the first local header (a self-extractor stub or other
    // prefix; none in real MSIX packages, but a valid ZIP may have one) are
    // described by no block hash: always fetch them, or `assemble` would leave
    // zeroes there and only the final SHA-256 would notice.
    let first_entry_offset = new_layout
        .files
        .first()
        .map_or(new_layout.central_directory_offset, |file| file.local_header_offset);
    if first_entry_offset > 0 {
        raw_fetch_spans.push((0, first_entry_offset));
    }

    for file in &new_layout.files {
        match &file.block_map_file {
            None => {
                // Not block-mapped (AppxBlockMap.xml itself, [Content_Types].xml,
                // AppxSignature.p7x, code-integrity catalogs, or any entry the
                // block map disagrees with) -- always fetch the whole record.
                raw_fetch_spans.push((file.local_header_offset, file.end_offset));
            }
            Some(block_map_file) => {
                // The Local File Header is small and its bytes are not
                // described by any block hash -- always fetch fresh.
                raw_fetch_spans.push((
                    file.local_header_offset,
                    file.local_header_offset + block_map_file.lfh_size,
                ));
                for block in &block_map_file.blocks {
                    total_blocks += 1;
                    let key = (block.hash_base64.clone(), block.size, block.stored);
                    if let Some(&base_offset) = base_reuse_index.get(&key) {
                        reused_blocks += 1;
                        reused_bytes += block.size;
                        copies.push(CopyStep {
                            base_offset,
                            new_offset: block.offset,
                            len: block.size,
                        });
                    } else {
                        raw_fetch_spans.push((block.offset, block.offset + block.size));
                    }
                }
                let closer_tail = file.closer_tail_len();
                if closer_tail > 0 {
                    let start = block_map_file.data_offset + block_map_file.block_data_size;
                    raw_fetch_spans.push((start, start + closer_tail));
                }
                let data_descriptor_len = file.data_descriptor_len();
                if data_descriptor_len > 0 {
                    let start = block_map_file.data_offset + file.compressed_size;
                    raw_fetch_spans.push((start, start + data_descriptor_len));
                }
            }
        }
    }

    // Central directory + (if present) ZIP64 EOCD locator/record + the
    // classic EOCD: everything from the first central-directory byte to EOF.
    raw_fetch_spans.push((new_layout.central_directory_offset, new_layout.file_size));

    let fetches = coalesce(&raw_fetch_spans, config.coalesce_gap);

    DeltaPlan {
        new_size: new_layout.file_size,
        copies,
        fetches,
        total_blocks,
        reused_blocks,
        reused_bytes,
    }
}

/// Merge any two spans whose gap is at most `gap` bytes into one. A gap that
/// happens to cover bytes another part of the plan intended to `copy` is
/// swallowed into the fetch on purpose: the fetched bytes are the new
/// package's real bytes for that range, so overlapping a copy is always
/// correct (just occasionally wasteful) regardless of which the executor
/// applies first.
fn coalesce(spans: &[(u64, u64)], gap: u64) -> Vec<FetchStep> {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable_by_key(|span| span.0);
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in sorted {
        if let Some(last) = merged.last_mut() {
            if start.saturating_sub(last.1) <= gap {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
        .into_iter()
        .map(|(offset, end)| FetchStep {
            offset,
            len: end - offset,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appx_blockmap::{AppxBlock, AppxBlockMap, AppxBlockMapFile};
    use crate::delta::layout::resolve_package_layout;
    use crate::delta::zip_format::{CentralDirectoryEntry, ZipLayout};

    fn cd_entry(name: &str, lho: u64, csize: u64, usize_: u64) -> CentralDirectoryEntry {
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

    fn block(hash: &str, size: u64) -> AppxBlock {
        AppxBlock {
            hash_base64: hash.to_string(),
            size,
            stored: false,
        }
    }

    /// One block-mapped entry filling the whole package, with no data
    /// descriptor and an 8-byte fake central directory right after the
    /// compressed data (dd_len == closer_tail == 0), so every byte in the
    /// resulting layout is accounted for by construction.
    fn single_file_layout(name: &str, lho: u64, lfh_size: u64, blocks: Vec<AppxBlock>) -> PackageLayout {
        const CENTRAL_DIRECTORY_SIZE: u64 = 8;
        let csize: u64 = blocks.iter().map(|b| b.size).sum();
        let data_offset = lho + lfh_size;
        let cd_offset = data_offset + csize;
        let file_size = cd_offset + CENTRAL_DIRECTORY_SIZE;
        let zip = ZipLayout {
            file_size,
            central_directory_offset: cd_offset,
            central_directory_size: CENTRAL_DIRECTORY_SIZE,
            entries: vec![cd_entry(name, lho, csize, csize * 2)],
        };
        let block_map = AppxBlockMap {
            files: vec![AppxBlockMapFile {
                name: name.replace('/', "\\"),
                uncompressed_size: csize * 2,
                lfh_size: Some(lfh_size),
                blocks,
            }],
        };
        resolve_package_layout(zip, Some(block_map)).unwrap()
    }

    #[test]
    fn reuses_identical_blocks_and_fetches_the_rest() {
        let base = single_file_layout(
            "app/a.bin",
            0,
            40,
            vec![block("h1", 100), block("h2", 200), block("h3", 300)],
        );
        // New package: same h1 and h3 (unchanged upstream bytes), h2 changed.
        let new_layout = single_file_layout(
            "app/a.bin",
            0,
            40,
            vec![block("h1", 100), block("h2-changed", 210), block("h3", 300)],
        );

        let reuse_index = build_reuse_index(&base);
        let plan = plan_delta(&reuse_index, &new_layout, &PlannerConfig { coalesce_gap: 0 });

        assert_eq!(plan.total_blocks, 3);
        assert_eq!(plan.reused_blocks, 2);
        assert_eq!(plan.reused_bytes, 400);
        assert_eq!(plan.copies.len(), 2);
        // Copies should point at the OLD file's byte offsets for those blocks.
        let h1_copy = plan.copies.iter().find(|c| c.len == 100).unwrap();
        assert_eq!(h1_copy.base_offset, 40); // base data starts at lho(0)+lfh_size(40)
        assert_eq!(h1_copy.new_offset, 40);

        // Fetches at gap=0: LFH (40B), changed h2 block (210B), and the
        // 8-byte central-directory tail -- three separate, non-adjacent spans.
        assert_eq!(plan.request_count(), 3);
        assert_eq!(plan.fetch_bytes(), 40 + 210 + 8);
        assert!(plan.savings_pct() > 0.0 && plan.savings_pct() < 100.0);
    }

    #[test]
    fn coalescing_merges_nearby_fetch_spans_into_fewer_requests() {
        let base = single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100), block("h2", 100)]);
        // h1 unchanged (reusable), h2 changed -- the LFH fetch and the
        // reused h1 block leave a gap before the h2-and-tail fetch.
        let new_layout = single_file_layout(
            "app/a.bin",
            0,
            40,
            vec![block("h1", 100), block("h2-changed", 100)],
        );
        let reuse_index = build_reuse_index(&base);

        let tight = plan_delta(&reuse_index, &new_layout, &PlannerConfig { coalesce_gap: 0 });
        // h2's fetch span (140..240) is already adjacent to the 8-byte
        // central-directory tail (240..248) even at gap=0, so tight still
        // merges those two but keeps the LFH (0..40) separate (gap=100 to
        // the next span clears any zero/near-zero threshold).
        assert_eq!(tight.request_count(), 2);

        let wide = plan_delta(
            &reuse_index,
            &new_layout,
            &PlannerConfig {
                coalesce_gap: 10_000,
            },
        );
        assert!(wide.request_count() < tight.request_count());
        assert_eq!(wide.request_count(), 1);
        // The single coalesced span covers everything from the LFH to EOF.
        assert_eq!(wide.fetches[0].offset, 0);
        assert_eq!(wide.fetches[0].len, new_layout.file_size);
    }

    #[test]
    fn unchanged_package_reuses_every_block_and_only_fetches_metadata() {
        let base = single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100), block("h2", 200)]);
        let new_layout =
            single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100), block("h2", 200)]);
        let reuse_index = build_reuse_index(&base);
        // A real-sized coalescing gap (e.g. the 256 KiB default) would swallow
        // this whole toy file into one span; gap=0 isolates the metadata-only
        // fetches this test is about.
        let plan = plan_delta(&reuse_index, &new_layout, &PlannerConfig { coalesce_gap: 0 });
        assert_eq!(plan.reused_blocks, 2);
        assert_eq!(plan.total_blocks, 2);
        // Only the LFH + central-directory tail are ever fetched.
        assert!(plan.fetch_bytes() < new_layout.file_size / 2);
        assert!(plan.savings_pct() > 50.0);
    }

    #[test]
    fn same_hash_but_different_on_disk_size_is_not_reused() {
        // In a real MSIX the block hash covers the *uncompressed* bytes, so
        // identical content re-compressed differently (or stored vs
        // deflated) keeps its hash and only the on-disk size changes.
        // Copying the base's bytes into that slot would produce the wrong
        // encoding, so the reuse key must include the size, not just the hash.
        let base = single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100), block("h2", 200)]);
        let new_layout = single_file_layout(
            "app/a.bin",
            0,
            40,
            // h1: same hash and size (reusable). h2: same hash, size differs.
            vec![block("h1", 100), block("h2", 150)],
        );
        let reuse_index = build_reuse_index(&base);
        assert!(reuse_index.contains_key(&("h2".to_string(), 200, false)));
        assert!(!reuse_index.contains_key(&("h2".to_string(), 150, false)));

        let plan = plan_delta(&reuse_index, &new_layout, &PlannerConfig { coalesce_gap: 0 });
        assert_eq!(plan.total_blocks, 2);
        assert_eq!(plan.reused_blocks, 1, "only the same-hash, same-size block is reused");
        assert_eq!(plan.copies.len(), 1);
        assert_eq!(plan.copies[0].len, 100);
        // The size-mismatched block (new offsets 140..290) is fetched whole.
        assert!(plan.fetches.iter().any(|f| f.offset <= 140 && f.offset + f.len >= 140 + 150));
    }

    #[test]
    fn same_hash_and_size_but_a_different_storage_mode_is_not_reused() {
        // A stored block and a deflated block of identical content can have
        // the same on-disk size for incompressible data, yet different bytes.
        let base = single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100)]);
        let mut new_layout = single_file_layout("app/a.bin", 0, 40, vec![block("h1", 100)]);
        new_layout.files[0].block_map_file.as_mut().unwrap().blocks[0].stored = true;
        let plan = plan_delta(&build_reuse_index(&base), &new_layout, &PlannerConfig { coalesce_gap: 0 });
        assert_eq!(plan.reused_blocks, 0);
    }

    #[test]
    fn bytes_before_the_first_entry_are_fetched() {
        // A package with a 64-byte prefix before its first local header.
        let base = single_file_layout("app/a.bin", 64, 40, vec![block("h1", 100)]);
        let new_layout = single_file_layout("app/a.bin", 64, 40, vec![block("h1", 100)]);
        let plan = plan_delta(&build_reuse_index(&base), &new_layout, &PlannerConfig { coalesce_gap: 0 });
        assert!(
            plan.fetches.iter().any(|f| f.offset == 0 && f.len >= 64),
            "the prefix 0..64 must be covered by a fetch: {:?}",
            plan.fetches
        );
        // Every byte is covered by a copy or a fetch.
        let mut covered = vec![false; new_layout.file_size as usize];
        for c in &plan.copies {
            covered[c.new_offset as usize..(c.new_offset + c.len) as usize].fill(true);
        }
        for f in &plan.fetches {
            covered[f.offset as usize..(f.offset + f.len) as usize].fill(true);
        }
        assert!(covered.iter().all(|c| *c));
    }

    #[test]
    fn ancillary_non_block_mapped_entries_are_always_fetched_whole() {
        let zip = ZipLayout {
            file_size: 2_000,
            central_directory_offset: 1_900,
            central_directory_size: 50,
            entries: vec![cd_entry("AppxBlockMap.xml", 0, 500, 900)],
        };
        let new_layout = resolve_package_layout(zip, None).unwrap();
        let plan = plan_delta(&HashMap::new(), &new_layout, &PlannerConfig { coalesce_gap: 0 });
        assert_eq!(plan.total_blocks, 0);
        assert_eq!(plan.copies.len(), 0);
        // Not block-mapped: the whole on-disk record [lho, end_offset) is
        // fetched, not just its declared compressed size -- here that's the
        // entire span up to the central directory (end_offset == cd_offset
        // for the only entry), plus the CD tail itself. No savings possible.
        assert_eq!(plan.fetch_bytes(), new_layout.file_size);
        assert_eq!(plan.savings_pct(), 0.0);
    }
}
