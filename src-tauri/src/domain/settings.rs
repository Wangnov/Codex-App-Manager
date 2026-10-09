use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub mirror_base_url: String,
    pub install_root: String,
    pub preserve_user_data_by_default: bool,
    pub disable_codex_self_updates: bool,
    pub codex_launch_arguments: String,
}

impl AppSettings {
    pub fn new(mirror_base_url: String, install_root: String) -> Self {
        Self {
            mirror_base_url,
            install_root,
            preserve_user_data_by_default: true,
            disable_codex_self_updates: false,
            codex_launch_arguments: String::new(),
        }
    }
}
