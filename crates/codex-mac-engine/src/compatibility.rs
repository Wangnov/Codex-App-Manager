//! Report an unsupported update without treating a successful check as an error.

use serde::Serialize;

use crate::Appcast;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacOsCompatibility {
    pub host_version: String,
    pub minimum_system_version: String,
    /// Highest-build item with an explicit, supported minimum OS requirement.
    /// None means the current feed does not establish a compatible release.
    pub latest_compatible_version: Option<String>,
}

fn version(value: &str) -> Option<[u32; 3]> {
    let mut result = [0; 3];
    let mut parts = value.trim().split('.');
    for item in &mut result {
        let Some(part) = parts.next() else {
            return Some(result);
        };
        *item = part.parse().ok()?;
    }
    parts.next().is_none().then_some(result)
}

pub fn update_compatibility(appcast: &Appcast, host_version: &str) -> Option<MacOsCompatibility> {
    let host = version(host_version)?;
    let required = appcast.latest()?.minimum_system_version.as_deref()?;
    if host >= version(required)? {
        return None;
    }
    let compatible = appcast
        .items
        .iter()
        .filter(|item| {
            item.minimum_system_version
                .as_deref()
                .and_then(version)
                .is_some_and(|minimum| host >= minimum)
        })
        .max_by_key(|item| item.build);
    Some(MacOsCompatibility {
        host_version: host_version.trim().to_string(),
        minimum_system_version: required.to_string(),
        latest_compatible_version: compatible.map(|item| item.short_version.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppcastItem, Enclosure};

    fn item(build: u64, minimum: Option<&str>) -> AppcastItem {
        AppcastItem {
            build,
            short_version: format!("v{build}"),
            minimum_system_version: minimum.map(str::to_owned),
            pub_date: None,
            full: Enclosure {
                url: "https://example.invalid/update.zip".into(),
                length: 1,
                ed_signature: None,
            },
            deltas: vec![],
        }
    }

    #[test]
    fn old_host_gets_highest_confirmed_compatible_build() {
        let appcast = Appcast {
            items: vec![
                item(30, Some("13")),
                item(10, Some("11")),
                item(20, Some("12.7")),
            ],
        };
        let result = update_compatibility(&appcast, "12.7.6").unwrap();
        assert_eq!(result.host_version, "12.7.6");
        assert_eq!(result.minimum_system_version, "13");
        assert_eq!(result.latest_compatible_version.as_deref(), Some("v20"));
        assert!(update_compatibility(&appcast, "13.0").is_none());
    }

    #[test]
    fn unknown_requirements_are_not_advertised_as_compatible() {
        let appcast = Appcast {
            items: vec![
                item(30, Some("13.3.1")),
                item(20, None),
                item(10, Some("unknown")),
            ],
        };
        assert!(update_compatibility(&appcast, "13.3")
            .unwrap()
            .latest_compatible_version
            .is_none());
        assert!(update_compatibility(&appcast, "13.3.1").is_none());
        assert!(update_compatibility(&appcast, "unknown").is_none());
    }
}
