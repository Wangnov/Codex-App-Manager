use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
use crate::process::{hidden_command, run_capturing, RunLimits};
use crate::EngineError;

// The mirror currently serves Store-re-signed MSIX packages; add a separate
// exact direct-signing anchor here if the Windows source changes in the future.
pub const OPENAI_MARKETPLACE_PUBLISHER_SUBJECT: &str =
    "cn=50bdfd77-8903-4850-9ffe-6e8522f64d5b";
#[cfg(any(windows, test))]
const MICROSOFT_MARKETPLACE_ISSUER_CN_PREFIX: &str = "cn=microsoft marketplace ca";
#[cfg(any(windows, test))]
const MICROSOFT_MARKETPLACE_ISSUER_ORG: &str = "o=microsoft corporation";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticodeReport {
    pub trusted: bool,
    pub publisher_is_openai: bool,
    pub status: String,
    pub status_message: String,
    pub subject: String,
    pub issuer: String,
    pub thumbprint: String,
}

impl AuthenticodeReport {
    pub fn is_valid_openai(&self) -> bool {
        self.trusted && self.publisher_is_openai
    }
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(any(windows, test))]
fn normalized_dn_components(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|component| component.trim().to_ascii_lowercase())
        .filter(|component| !component.is_empty())
        .collect()
}

#[cfg(any(windows, test))]
fn has_pinned_marketplace_issuer(issuer: &str) -> bool {
    let components = normalized_dn_components(issuer);
    components
        .iter()
        .any(|component| component.starts_with(MICROSOFT_MARKETPLACE_ISSUER_CN_PREFIX))
        && components
            .iter()
            .any(|component| component == MICROSOFT_MARKETPLACE_ISSUER_ORG)
}

#[cfg(any(windows, test))]
fn is_pinned_openai_publisher(subject: &str, issuer: &str) -> bool {
    let subject = subject.trim().to_ascii_lowercase();
    subject == OPENAI_MARKETPLACE_PUBLISHER_SUBJECT && has_pinned_marketplace_issuer(issuer)
}

#[cfg(any(windows, test))]
fn report_from_json(json: &str) -> Result<AuthenticodeReport, EngineError> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| EngineError::Authenticode(format!("PowerShell JSON: {e}")))?;
    let s = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let status = s("status");
    let subject = s("subject");
    let issuer = s("issuer");
    let publisher_is_openai = is_pinned_openai_publisher(&subject, &issuer);

    Ok(AuthenticodeReport {
        trusted: status.eq_ignore_ascii_case("Valid"),
        publisher_is_openai,
        status,
        status_message: s("statusMessage"),
        subject,
        issuer,
        thumbprint: s("thumbprint"),
    })
}

#[cfg(windows)]
fn powershell_exe() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .map(|windir| {
            windir
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

// PowerShell custom-object casts are rejected by ConstrainedLanguage. A plain
// hashtable serializes to the same JSON shape while remaining usable under
// AppLocker / WDAC-managed sessions.
#[cfg(windows)]
const AUTHENTICODE_REPORT_PROJECTION: &str = r#"
@{
  status = [string]$sig.Status
  statusMessage = [string]$sig.StatusMessage
  subject = if ($sig.SignerCertificate) { [string]$sig.SignerCertificate.Subject } else { '' }
  issuer = if ($sig.SignerCertificate) { [string]$sig.SignerCertificate.Issuer } else { '' }
  thumbprint = if ($sig.SignerCertificate) { [string]$sig.SignerCertificate.Thumbprint } else { '' }
} | ConvertTo-Json -Compress
"#;

#[cfg(windows)]
fn authenticode_script(path: &Path) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$securityModule = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1'
Import-Module $securityModule -ErrorAction Stop
$sig = Get-AuthenticodeSignature -LiteralPath {path}
{report_projection}
"#,
        path = ps_quote(&path.to_string_lossy()),
        report_projection = AUTHENTICODE_REPORT_PROJECTION,
    )
}

#[cfg(windows)]
pub fn verify_openai_authenticode(path: &Path) -> Result<AuthenticodeReport, EngineError> {
    log::info!("Authenticode verification start");
    let script = authenticode_script(path);

    let mut command = hidden_command(powershell_exe());
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = run_capturing(command, RunLimits::probe(), None).map_err(|e| {
        EngineError::Authenticode(format!("Get-AuthenticodeSignature: {}", e.message()))
    })?;

    if !output.status.success() {
        let err = EngineError::Authenticode(format!(
            "Get-AuthenticodeSignature failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        log::error!("Authenticode verification failed error={err}");
        return Err(err);
    }

    let report = report_from_json(String::from_utf8_lossy(&output.stdout).trim())?;
    if report.is_valid_openai() {
        let signer = &report.subject;
        log::info!("Authenticode verification passed signer={signer}");
    } else {
        log::error!(
            "Authenticode verification failed error=status={} subject={}",
            report.status,
            report.subject
        );
    }
    Ok(report)
}

#[cfg(not(windows))]
pub fn verify_openai_authenticode(_path: &Path) -> Result<AuthenticodeReport, EngineError> {
    log::info!("Authenticode verification start");
    log::error!("Authenticode verification failed error=unsupported-platform");
    Ok(AuthenticodeReport {
        trusted: false,
        publisher_is_openai: false,
        status: "unsupported-platform".to_string(),
        status_message: "Authenticode verification is only available on Windows".to_string(),
        subject: String::new(),
        issuer: String::new(),
        thumbprint: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::report_from_json;

    #[test]
    fn rejects_direct_openai_subject_without_marketplace_anchor() {
        let report = report_from_json(
            r#"{
              "status":"Valid",
              "statusMessage":"Signature verified.",
              "subject":"CN=OpenAI OpCo, LLC, O=OpenAI OpCo, LLC, C=US",
              "issuer":"CN=Trusted CA",
              "thumbprint":"ABC"
            }"#,
        )
        .unwrap();
        assert!(!report.is_valid_openai());
    }

    #[test]
    fn accepts_current_store_publisher_guid_for_codex() {
        let report = report_from_json(
            r#"{
              "status":"Valid",
              "statusMessage":"Signature verified.",
              "subject":"CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B",
              "issuer":"CN=Microsoft Marketplace CA G 028, O=Microsoft Corporation, C=US",
              "thumbprint":"ABC"
            }"#,
        )
        .unwrap();
        assert!(report.is_valid_openai());
    }

    #[test]
    fn rejects_marketplace_subject_with_wrong_issuer() {
        let report = report_from_json(
            r#"{
              "status":"Valid",
              "statusMessage":"Signature verified.",
              "subject":"CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B",
              "issuer":"CN=Contoso Marketplace CA, O=Contoso, C=US",
              "thumbprint":"ABC"
            }"#,
        )
        .unwrap();
        assert!(!report.is_valid_openai());
    }

    #[cfg(windows)]
    #[test]
    fn authenticode_script_runs_in_constrained_language() {
        let path = std::env::current_exe().expect("resolve current test executable");
        let production_script = super::authenticode_script(&path);
        let script = format!(
            r#"$ErrorActionPreference = 'Stop'
$ExecutionContext.SessionState.LanguageMode = 'ConstrainedLanguage'
if ($ExecutionContext.SessionState.LanguageMode -ne 'ConstrainedLanguage') {{
  throw 'failed to enter ConstrainedLanguage'
}}
{production_script}
"#,
            production_script = production_script,
        );
        let output = super::hidden_command(super::powershell_exe())
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("run constrained-language Authenticode script");
        assert!(
            output.status.success(),
            "Authenticode script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = report_from_json(String::from_utf8_lossy(&output.stdout).trim())
            .expect("parse constrained-language Authenticode report");
        assert!(!report.status.is_empty());
    }
}
