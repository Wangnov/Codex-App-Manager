//! Low-level `AppxBlockMap.xml` parsing shared by the portable-install
//! extractor (`portable.rs`, which only needs each payload file's logical
//! path + uncompressed size) and the block-level delta engine (`delta/`,
//! which additionally needs each file's local-header size and per-block
//! compressed size + hash so it can locate individual blocks inside the
//! surrounding ZIP container without ever reading a local file header).
//!
//! Kept independent of both callers: it has no opinion on Windows path
//! safety (that stays in `portable.rs`) and no opinion on ZIP byte layout
//! (that stays in `delta::zip_format`).

use crate::EngineError;

/// One 64 KiB (or shorter, for the final block of a file) chunk inside a
/// block-compressed MSIX payload file, as declared by `<Block>` in
/// `AppxBlockMap.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppxBlock {
    /// Base64 `Hash` attribute — SHA-256 of the block's *uncompressed*
    /// content (confirmed by independently re-inflating real blocks and
    /// comparing hashes; this is why two blocks with the same hash *and* the
    /// same on-disk `size` are, in practice, byte-identical once compressed
    /// with the same tool/settings — that equivalence is what the delta
    /// planner relies on to reuse bytes without decompressing anything).
    pub hash_base64: String,
    /// On-disk (compressed, or raw if `stored`) byte size of this block.
    pub size: u64,
    /// `true` when the XML omitted `Size` (a stored/uncompressed block),
    /// meaning `size` was derived rather than read directly.
    pub stored: bool,
}

/// One `<File>` element: a single payload file inside the MSIX/APPX package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppxBlockMapFile {
    /// Logical path exactly as written in the XML: backslash-separated, not
    /// percent-encoded (unlike the ZIP entry name for the same file, which
    /// MakeAppx percent-encodes for reserved characters such as `@`).
    pub name: String,
    pub uncompressed_size: u64,
    /// Local File Header size for this entry (fixed 30-byte header + file
    /// name + extra field), as recorded by the packer. Combined with the
    /// entry's local-header offset from the ZIP central directory, this
    /// gives the exact start of the entry's compressed data with zero extra
    /// reads — no need to ever fetch/parse the local header itself.
    ///
    /// `None` when the XML's `<File>` element omits `LfhSize` -- optional
    /// here because only `delta::layout` needs it; `portable.rs`'s
    /// extractor never reads this field and must keep accepting a block map
    /// that omits it, exactly as it did before this parser was shared.
    /// `delta::layout` treats a missing value as its own hard error.
    pub lfh_size: Option<u64>,
    pub blocks: Vec<AppxBlock>,
}

impl AppxBlockMapFile {
    /// Sum of on-disk block sizes. For a fully block-compressed entry this
    /// equals the ZIP central directory's compressed size; any excess in the
    /// real compressed size over this sum is the small tail the block map
    /// does not describe (see `delta::layout`'s `closer_tail`).
    pub fn block_data_size(&self) -> u64 {
        self.blocks.iter().map(|block| block.size).sum()
    }
}

/// A fully parsed `AppxBlockMap.xml`.
#[derive(Debug, Clone, Default)]
pub struct AppxBlockMap {
    pub files: Vec<AppxBlockMapFile>,
}

/// Default APPX block size (64 KiB). Only used to size the final block of a
/// *stored* (uncompressed) file, whose `<Block>` omits `Size`.
pub const DEFAULT_APPX_BLOCK_SIZE: u64 = 65536;

fn attr<'a>(node: &roxmltree::Node<'a, 'a>, name: &str, context: &str) -> Result<&'a str, EngineError> {
    node.attribute(name)
        .ok_or_else(|| EngineError::Msix(format!("AppxBlockMap {context} missing {name}")))
}

fn parse_u64_attr(value: &str, context: &str) -> Result<u64, EngineError> {
    value
        .parse::<u64>()
        .map_err(|err| EngineError::Msix(format!("AppxBlockMap {context} has invalid integer: {err}")))
}

/// Parse a single `<File>` element's `LfhSize` + `<Block>` children --
/// everything only `delta::layout` needs, never `portable.rs`'s extractor.
/// Any malformed or invalid data here (a nonnumeric `LfhSize`, a `<Block>`
/// missing `Hash`, a nonnumeric `Block Size`) downgrades to `(None, vec![])`
/// instead of failing the whole document: this field-set existing in some
/// form but being unusable for delta purposes is exactly the same case, as
/// far as `delta::layout` is concerned, as `LfhSize`/`<Block>` being absent
/// entirely (its `lfh_size.ok_or_else` already errors on `None`, which is
/// the correct outcome -- fall back to a full download for *that* package --
/// without portable extraction, which never reads either field, ever seeing
/// the error at all).
fn parse_delta_only_fields(file: &roxmltree::Node, name: &str, uncompressed_size: u64) -> (Option<u64>, Vec<AppxBlock>) {
    let parse = || -> Result<(Option<u64>, Vec<AppxBlock>), EngineError> {
        let lfh_size = file
            .attribute("LfhSize")
            .map(|raw| parse_u64_attr(raw, &format!("File LfhSize: {name}")))
            .transpose()?;

        let mut blocks = Vec::new();
        for (index, block) in file
            .children()
            .filter(|node| node.has_tag_name("Block"))
            .enumerate()
        {
            let hash_base64 = attr(&block, "Hash", "Block")?.to_string();
            let (size, stored) = match block.attribute("Size") {
                Some(raw) => (parse_u64_attr(raw, &format!("Block Size: {name}"))?, false),
                None => {
                    // A stored (uncompressed) block omits Size: its length is
                    // implied — a full 64 KiB block, except possibly the last
                    // one, which is whatever remains of the uncompressed file.
                    let consumed = index as u64 * DEFAULT_APPX_BLOCK_SIZE;
                    let remaining = uncompressed_size.saturating_sub(consumed);
                    (remaining.min(DEFAULT_APPX_BLOCK_SIZE), true)
                }
            };
            blocks.push(AppxBlock {
                hash_base64,
                size,
                stored,
            });
        }
        Ok((lfh_size, blocks))
    };
    parse().unwrap_or((None, Vec::new()))
}

/// Parse `AppxBlockMap.xml` into the full per-file, per-block structure.
///
/// This is the single place that walks the XML; both `portable.rs` (which
/// only wants `name` + `uncompressed_size`) and `delta::layout` (which also
/// needs `lfh_size` and every block) call through here so the traversal and
/// its error messages exist exactly once. Only `Name` and `Size` -- the two
/// fields the portable extractor actually reads -- are hard requirements
/// for a `<File>` element, matching the standalone extractor this replaced;
/// see [`parse_delta_only_fields`] for why everything else degrades instead
/// of failing the whole parse.
pub fn parse_appx_block_map_xml(xml: &str) -> Result<AppxBlockMap, EngineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| EngineError::Msix(format!("AppxBlockMap.xml: {err}")))?;

    let mut files = Vec::new();
    for file in document
        .descendants()
        .filter(|node| node.has_tag_name("File"))
    {
        let name = attr(&file, "Name", "File")?.to_string();
        let uncompressed_size = parse_u64_attr(attr(&file, "Size", "File")?, &format!("File Size: {name}"))?;
        let (lfh_size, blocks) = parse_delta_only_fields(&file, &name, uncompressed_size);

        files.push(AppxBlockMapFile {
            name,
            uncompressed_size,
            lfh_size,
            blocks,
        });
    }

    if files.is_empty() {
        return Err(EngineError::Msix(
            "AppxBlockMap.xml contains no payload files".to_string(),
        ));
    }

    Ok(AppxBlockMap { files })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap" HashMethod="http://www.w3.org/2001/04/xmlenc#sha256">
  <File Name="app\resources\app.asar" Size="140000" LfhSize="47">
    <Block Hash="aaaa" Size="65536" />
    <Block Hash="bbbb" Size="60000" />
    <Block Hash="cccc" Size="14464" />
  </File>
  <File Name="app\resources\plain.txt" Size="10" LfhSize="41">
    <Block Hash="dddd" />
  </File>
</BlockMap>"#;

    #[test]
    fn parses_files_and_blocks_with_declared_sizes() {
        let parsed = parse_appx_block_map_xml(SAMPLE).unwrap();
        assert_eq!(parsed.files.len(), 2);
        let asar = &parsed.files[0];
        assert_eq!(asar.name, r"app\resources\app.asar");
        assert_eq!(asar.uncompressed_size, 140000);
        assert_eq!(asar.lfh_size, Some(47));
        assert_eq!(asar.blocks.len(), 3);
        assert_eq!(asar.blocks[0].size, 65536);
        assert!(!asar.blocks[0].stored);
        assert_eq!(asar.block_data_size(), 65536 + 60000 + 14464);
    }

    #[test]
    fn derives_stored_block_size_from_uncompressed_length() {
        let parsed = parse_appx_block_map_xml(SAMPLE).unwrap();
        let plain = &parsed.files[1];
        assert_eq!(plain.blocks.len(), 1);
        assert!(plain.blocks[0].stored);
        assert_eq!(plain.blocks[0].size, 10);
    }

    #[test]
    fn accepts_missing_lfh_size_as_none() {
        // The portable-install extractor never needs `LfhSize` and must
        // keep accepting a block map that omits it, exactly as it did
        // before this parser was shared with the delta engine (which is
        // the caller that actually requires the field -- see
        // `delta::layout`).
        let xml = r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap"><File Name="a" Size="1"><Block Hash="x" Size="1"/></File></BlockMap>"#;
        let parsed = parse_appx_block_map_xml(xml).unwrap();
        assert_eq!(parsed.files[0].lfh_size, None);
    }

    #[test]
    fn nonnumeric_lfh_size_degrades_to_none_instead_of_failing_the_document() {
        // Portable extraction never reads `LfhSize`; a document with a
        // malformed value for it (or, as below, no parseable blocks) must
        // still parse successfully for `Name`/`Size` -- exactly as it would
        // if the attribute were absent. Only `delta::layout`, which does
        // need the field, treats the resulting `None` as its own error.
        let xml = r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap"><File Name="a" Size="10" LfhSize="not-a-number"><Block Hash="x" Size="10"/></File></BlockMap>"#;
        let parsed = parse_appx_block_map_xml(xml).unwrap();
        assert_eq!(parsed.files[0].name, "a");
        assert_eq!(parsed.files[0].uncompressed_size, 10);
        assert_eq!(parsed.files[0].lfh_size, None);
    }

    #[test]
    fn a_block_missing_hash_degrades_the_whole_file_to_no_blocks_instead_of_failing() {
        // The pre-refactor portable extractor never even looked at `<Block>`
        // elements, so a document with one malformed `<Block>` (here,
        // missing `Hash`) must not become unparseable for portable
        // extraction just because the shared parser also reads blocks now.
        let xml = r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap"><File Name="a" Size="10" LfhSize="1"><Block Size="10"/></File></BlockMap>"#;
        let parsed = parse_appx_block_map_xml(xml).unwrap();
        assert_eq!(parsed.files[0].name, "a");
        assert_eq!(parsed.files[0].uncompressed_size, 10);
        // The whole delta-only field set for this file -- LfhSize included
        // -- downgrades together, so `delta::layout` (which needs both)
        // fails closed on the missing LfhSize rather than resolving blocks
        // against a value that was never actually validated.
        assert_eq!(parsed.files[0].lfh_size, None);
        assert!(parsed.files[0].blocks.is_empty());
    }

    #[test]
    fn rejects_empty_block_map() {
        let xml = r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap"></BlockMap>"#;
        let err = parse_appx_block_map_xml(xml).unwrap_err();
        assert!(err.to_string().contains("no payload files"), "{err}");
    }
}
