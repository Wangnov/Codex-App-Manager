//! Update planning: given a parsed appcast and the installed build number,
//! decide whether to apply a binary delta (preferred, tiny) or download the
//! full archive (fallback when no delta covers the installed build).

use serde::Serialize;

use crate::appcast::Appcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UpdateStrategy {
    /// Apply OpenAI's Sparkle binary delta from `from_build` to latest.
    Delta { from_build: u64 },
    /// Download the full archive (no delta covers the installed build, or it is
    /// outside the published delta window).
    Full,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlan {
    pub up_to_date: bool,
    pub current_build: u64,
    pub latest_build: u64,
    pub latest_short_version: String,
    pub strategy: UpdateStrategy,
    /// What we would actually download (delta or full archive).
    pub download_url: String,
    pub download_size: u64,
    pub ed_signature: Option<String>,
    /// Size of the full archive, for comparison / savings display.
    pub full_size: u64,
    /// Percentage saved vs downloading the full archive (0..=100).
    pub savings_pct: f64,
}

/// Build an [`UpdatePlan`] for the given installed build number.
///
/// Returns `None` only if the appcast has no items.
pub fn plan_update(appcast: &Appcast, current_build: u64) -> Option<UpdatePlan> {
    let latest = appcast.latest()?;
    let full_size = latest.full.length;

    if current_build >= latest.build {
        log::debug!(
            "macOS plan result strategy=none current_build={current_build} latest_build={} download_size=0 full_size={full_size}",
            latest.build
        );
        return Some(UpdatePlan {
            up_to_date: true,
            current_build,
            latest_build: latest.build,
            latest_short_version: latest.short_version.clone(),
            strategy: UpdateStrategy::Full,
            download_url: latest.full.url.clone(),
            download_size: 0,
            ed_signature: latest.full.ed_signature.clone(),
            full_size,
            savings_pct: 0.0,
        });
    }

    // Prefer a delta published *from* the installed build.
    let (strategy, url, size, sig) = match latest
        .deltas
        .iter()
        .find(|d| d.from_build == current_build)
    {
        Some(d) => (
            UpdateStrategy::Delta {
                from_build: d.from_build,
            },
            d.url.clone(),
            d.length,
            d.ed_signature.clone(),
        ),
        None => (
            UpdateStrategy::Full,
            latest.full.url.clone(),
            latest.full.length,
            latest.full.ed_signature.clone(),
        ),
    };

    let savings_pct = if full_size > 0 {
        (1.0 - (size as f64 / full_size as f64)) * 100.0
    } else {
        0.0
    };

    let strategy_name = match &strategy {
        UpdateStrategy::Delta { .. } => "delta",
        UpdateStrategy::Full => "full",
    };
    log::debug!(
        "macOS plan result strategy={strategy_name} current_build={current_build} latest_build={} download_size={size} full_size={full_size}",
        latest.build
    );

    Some(UpdatePlan {
        up_to_date: false,
        current_build,
        latest_build: latest.build,
        latest_short_version: latest.short_version.clone(),
        strategy,
        download_url: url,
        download_size: size,
        ed_signature: sig,
        full_size,
        savings_pct,
    })
}
