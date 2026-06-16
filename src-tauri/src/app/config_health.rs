use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigStatus {
    #[default]
    Ok,
    Recovered,
    Corrupt,
}

impl ConfigStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Recovered => "recovered",
            Self::Corrupt => "corrupt",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StoreLoadHealth {
    pub status: ConfigStatus,
    pub unknown_source: Option<String>,
    pub detail: Option<String>,
}

impl StoreLoadHealth {
    pub fn ok() -> Self {
        Self::default()
    }

    pub fn recovered(detail: String) -> Self {
        Self {
            status: ConfigStatus::Recovered,
            detail: Some(detail),
            ..Self::default()
        }
    }

    pub fn corrupt(detail: String) -> Self {
        Self {
            status: ConfigStatus::Corrupt,
            detail: Some(detail),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHealth {
    pub settings_status: String,
    pub provenance_status: String,
    pub unknown_source: Option<String>,
    pub detail: Option<String>,
}

impl ConfigHealth {
    pub fn from_parts(settings: StoreLoadHealth, provenance: StoreLoadHealth) -> Self {
        let detail = [settings.detail, provenance.detail]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("；");
        Self {
            settings_status: settings.status.as_str().to_string(),
            provenance_status: provenance.status.as_str().to_string(),
            unknown_source: settings.unknown_source,
            detail: (!detail.is_empty()).then_some(detail),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.settings_status == "ok"
            && self.provenance_status == "ok"
            && self.unknown_source.is_none()
    }
}
