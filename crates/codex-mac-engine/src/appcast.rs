//! Sparkle appcast model + parser.
//!
//! We match elements/attributes by *local name* (ignoring the `sparkle:` XML
//! namespace prefix) so the parser is robust to prefix/namespace variations.

use serde::Serialize;

use crate::EngineError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enclosure {
    pub url: String,
    pub length: u64,
    pub ed_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    /// `sparkle:deltaFrom` — the installed build this delta upgrades *from*.
    pub from_build: u64,
    pub url: String,
    pub length: u64,
    pub ed_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppcastItem {
    /// `sparkle:version` — the monotonic build number (e.g. 3575).
    pub build: u64,
    /// `sparkle:shortVersionString` — marketing version (e.g. "26.602.30954").
    pub short_version: String,
    pub minimum_system_version: Option<String>,
    /// RSS `<pubDate>` (RFC-822) — when this build was published, if the feed
    /// carries it. The UI formats it as the version's release date.
    pub pub_date: Option<String>,
    /// The full update archive (`.zip`).
    pub full: Enclosure,
    /// Binary deltas from recent prior builds.
    pub deltas: Vec<Delta>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Appcast {
    pub items: Vec<AppcastItem>,
}

impl Appcast {
    /// The item with the highest build number.
    pub fn latest(&self) -> Option<&AppcastItem> {
        self.items.iter().max_by_key(|i| i.build)
    }
}

pub fn parse_appcast(xml: &str) -> Result<Appcast, EngineError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| EngineError::Parse(e.to_string()))?;

    let mut items = Vec::new();
    for item in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "item")
    {
        let mut build: Option<u64> = None;
        let mut short_version: Option<String> = None;
        let mut minimum_system_version: Option<String> = None;
        let mut pub_date: Option<String> = None;
        let mut full: Option<Enclosure> = None;
        let mut deltas: Vec<Delta> = Vec::new();

        let mut enclosure_build: Option<u64> = None;
        for child in item.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "version" => build = child.text().and_then(parse_u64).or(build),
                "shortVersionString" => {
                    short_version = child.text().map(|t| t.trim().to_string())
                }
                "minimumSystemVersion" => {
                    minimum_system_version = child.text().map(|t| t.trim().to_string())
                }
                "pubDate" => pub_date = child.text().map(|t| t.trim().to_string()),
                // A direct <enclosure> child of <item> is the full archive.
                "enclosure" => {
                    enclosure_build =
                        enclosure_build.or_else(|| attr(&child, "version").and_then(parse_u64));
                    full = Some(parse_enclosure(&child));
                }
                // <sparkle:deltas> wraps per-from-version delta enclosures.
                "deltas" => {
                    for d in child
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == "enclosure")
                    {
                        if let Some(delta) = parse_delta(&d) {
                            deltas.push(delta);
                        }
                    }
                }
                _ => {}
            }
        }

        // Some feeds carry the build only as `sparkle:version` on the full
        // enclosure rather than as a child element.
        let build = build.or(enclosure_build);

        if let (Some(build), Some(full)) = (build, full) {
            items.push(AppcastItem {
                build,
                short_version: short_version.unwrap_or_default(),
                minimum_system_version,
                pub_date,
                full,
                deltas,
            });
        }
    }

    if items.is_empty() {
        return Err(EngineError::EmptyAppcast);
    }
    Ok(Appcast { items })
}

fn parse_u64(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

fn attr<'a>(node: &roxmltree::Node<'a, 'a>, local: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

fn parse_enclosure(n: &roxmltree::Node) -> Enclosure {
    Enclosure {
        url: attr(n, "url").unwrap_or_default().to_string(),
        length: attr(n, "length").and_then(parse_u64).unwrap_or(0),
        ed_signature: attr(n, "edSignature").map(|s| s.to_string()),
    }
}

fn parse_delta(n: &roxmltree::Node) -> Option<Delta> {
    let from_build = attr(n, "deltaFrom").and_then(parse_u64)?;
    Some(Delta {
        from_build,
        url: attr(n, "url").unwrap_or_default().to_string(),
        length: attr(n, "length").and_then(parse_u64).unwrap_or(0),
        ed_signature: attr(n, "edSignature").map(|s| s.to_string()),
    })
}
