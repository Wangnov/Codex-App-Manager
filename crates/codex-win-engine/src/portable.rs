use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app_version::read_codex_app_version_from_install_root;
use crate::msix::{parse_appx_manifest_xml, MsixIdentity};
use crate::process::hidden_command;
use crate::EngineError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableInstallReport {
    pub success: bool,
    pub install_root: String,
    pub executable_path: Option<String>,
    pub version: String,
    pub backup_path: Option<String>,
    pub shortcut_created: bool,
    pub uninstall_entry_created: bool,
    pub relaunched: bool,
    pub message: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableUninstallReport {
    pub success: bool,
    #[serde(default)]
    pub partial: bool,
    pub install_root: String,
    pub removed_files: bool,
    pub removed_shortcut: bool,
    pub removed_uninstall_entry: bool,
    pub purged_user_data: bool,
    pub message: String,
    pub notes: Vec<String>,
}

struct PreparedPortable {
    payload_dir: PathBuf,
    identity: MsixIdentity,
}

fn io_err(context: &str, err: impl ToString) -> EngineError {
    EngineError::Io(format!("{context}: {}", err.to_string()))
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<(), EngineError> {
    fs::create_dir_all(to).map_err(|e| io_err("create dir", e))?;
    for entry in fs::read_dir(from).map_err(|e| io_err("read dir", e))? {
        let entry = entry.map_err(|e| io_err("read dir entry", e))?;
        let ty = entry.file_type().map_err(|e| io_err("read file type", e))?;
        let dest = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| io_err("copy file", e))?;
        }
    }
    Ok(())
}

fn extract_msix(msix_path: &Path, dest: &Path) -> Result<String, EngineError> {
    let file = fs::File::open(msix_path).map_err(|e| io_err("open MSIX", e))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| EngineError::Msix(format!("open zip: {e}")))?;
    let mut manifest_xml = None;

    for idx in 0..zip.len() {
        let mut file = zip
            .by_index(idx)
            .map_err(|e| EngineError::Msix(format!("read zip entry {idx}: {e}")))?;
        let Some(enclosed) = file.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(&enclosed);
        if file.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| io_err("create extracted dir", e))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err("create extracted parent", e))?;
        }
        let mut out =
            fs::File::create(&out_path).map_err(|e| io_err("create extracted file", e))?;
        std::io::copy(&mut file, &mut out).map_err(|e| io_err("extract file", e))?;

        if enclosed
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("AppxManifest.xml"))
            && enclosed.components().count() == 1
        {
            let mut xml = String::new();
            fs::File::open(&out_path)
                .and_then(|mut f| f.read_to_string(&mut xml))
                .map_err(|e| io_err("read extracted AppxManifest.xml", e))?;
            manifest_xml = Some(xml);
        }
    }

    manifest_xml.ok_or_else(|| EngineError::Msix("MSIX missing AppxManifest.xml".to_string()))
}

fn find_codex_exe(root: &Path) -> Result<PathBuf, EngineError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| io_err("walk extracted MSIX", e))? {
            let entry = entry.map_err(|e| io_err("walk extracted MSIX entry", e))?;
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| io_err("read file type", e))?;
            if ty.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("Codex.exe"))
            {
                return Ok(path);
            }
        }
    }
    Err(EngineError::Msix(
        "MSIX did not contain Codex.exe".to_string(),
    ))
}

fn prepare_portable_payload(
    msix_path: &Path,
    work_dir: &Path,
) -> Result<PreparedPortable, EngineError> {
    let extracted = work_dir.join("extracted");
    let payload = work_dir.join("payload");
    if extracted.exists() {
        fs::remove_dir_all(&extracted).map_err(|e| io_err("clear extracted dir", e))?;
    }
    if payload.exists() {
        fs::remove_dir_all(&payload).map_err(|e| io_err("clear payload dir", e))?;
    }
    fs::create_dir_all(&extracted).map_err(|e| io_err("create extracted dir", e))?;

    let manifest_xml = extract_msix(msix_path, &extracted)?;
    let identity = parse_appx_manifest_xml(&manifest_xml)?;
    let exe = find_codex_exe(&extracted)?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| EngineError::Msix("Codex.exe had no parent directory".to_string()))?;

    copy_dir_all(exe_dir, &payload)?;
    fs::write(payload.join("AppxManifest.xml"), manifest_xml)
        .map_err(|e| io_err("write portable AppxManifest.xml", e))?;

    Ok(PreparedPortable {
        payload_dir: payload,
        identity,
    })
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

#[cfg(windows)]
fn run_powershell(script: &str) -> Result<String, EngineError> {
    let output = hidden_command(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| EngineError::Install(format!("spawn powershell: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Install(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(windows)]
fn request_codex_close_filtered(timeout_secs: u64, root: Option<&Path>) -> Result<(), EngineError> {
    let root_filter = root
        .map(|path| ps_quote(&path.to_string_lossy()))
        .unwrap_or_else(|| "$null".to_string());
    let timeout = timeout_secs;
    let script = format!(
        r#"
$RootFilter = {root_filter}
if ($null -ne $RootFilter) {{
  try {{ $RootFilter = [System.IO.Path]::GetFullPath($RootFilter).TrimEnd('\') }} catch {{}}
}}
function Get-TargetCodexProcess {{
  $all = Get-Process -Name Codex -ErrorAction SilentlyContinue
  foreach ($p in $all) {{
    if ($null -eq $RootFilter) {{
      $p
      continue
    }}
    try {{
      $path = [string]$p.Path
      if (-not $path) {{ continue }}
      $full = [System.IO.Path]::GetFullPath($path)
      if ($full.Equals($RootFilter, [System.StringComparison]::OrdinalIgnoreCase) -or
          $full.StartsWith($RootFilter + '\', [System.StringComparison]::OrdinalIgnoreCase)) {{
        $p
      }}
    }} catch {{}}
  }}
}}
$deadline = (Get-Date).AddSeconds({timeout})
$procs = @(Get-TargetCodexProcess)
if ($procs.Count -eq 0) {{
  'no-targets'
  exit 0
}}
$targetIds = @($procs | ForEach-Object {{ $_.Id }})
foreach ($p in $procs) {{
  try {{
    if ($p.MainWindowHandle -ne 0) {{ [void]$p.CloseMainWindow() }}
  }} catch {{}}
}}
while ((Get-Date) -lt $deadline) {{
  Start-Sleep -Milliseconds 250
  $remaining = @()
  foreach ($id in $targetIds) {{
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($null -ne $p) {{ $remaining += $p }}
  }}
  if ($remaining.Count -eq 0) {{
    'closed'
    exit 0
  }}
}}
$forceIds = @($remaining | ForEach-Object {{ $_.Id }})
foreach ($id in $forceIds) {{
  try {{ Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }} catch {{}}
}}
$forceDeadline = (Get-Date).AddSeconds(5)
while ((Get-Date) -lt $forceDeadline) {{
  Start-Sleep -Milliseconds 250
  $remaining = @()
  foreach ($id in $forceIds) {{
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($null -ne $p) {{ $remaining += $p }}
  }}
  if ($remaining.Count -eq 0) {{
    'force-closed:' + ($forceIds -join ',')
    exit 0
  }}
}}
'running:' + (($remaining | ForEach-Object {{ $_.Id }}) -join ',')
"#
    );
    let result = run_powershell(&script)?;
    let trimmed = result.trim();
    if trimmed.ends_with("closed") || trimmed.ends_with("no-targets") {
        Ok(())
    } else if trimmed.starts_with("force-closed:") {
        log::warn!("target Codex processes required forced close result={trimmed}");
        Ok(())
    } else {
        Err(EngineError::Install(
            format!(
                "target Codex process is still running after graceful close request ({result}); no files were replaced"
            ),
        ))
    }
}

#[cfg(windows)]
fn request_codex_close(timeout_secs: u64) -> Result<(), EngineError> {
    request_codex_close_filtered(timeout_secs, None)
}

#[cfg(windows)]
fn request_codex_close_for_root(timeout_secs: u64, root: &Path) -> Result<(), EngineError> {
    request_codex_close_filtered(timeout_secs, Some(root))
}

#[cfg(not(windows))]
fn request_codex_close(_timeout_secs: u64) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(not(windows))]
fn request_codex_close_for_root(_timeout_secs: u64, _root: &Path) -> Result<(), EngineError> {
    Ok(())
}

pub fn close_codex_gracefully(timeout_secs: u64) -> Result<(), EngineError> {
    request_codex_close(timeout_secs)
}

pub fn close_codex_gracefully_for_root(timeout_secs: u64, root: &Path) -> Result<(), EngineError> {
    request_codex_close_for_root(timeout_secs, root)
}

#[cfg(windows)]
fn create_start_menu_shortcut(install_root: &Path) -> Result<bool, EngineError> {
    let exe = install_root.join("Codex.exe");
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(false);
    };
    let shortcut = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Codex.lnk");
    let script = format!(
        r#"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut({shortcut})
$shortcut.TargetPath = {target}
$shortcut.WorkingDirectory = {workdir}
$shortcut.IconLocation = {icon}
$shortcut.Save()
"#,
        shortcut = ps_quote(&shortcut.to_string_lossy()),
        target = ps_quote(&exe.to_string_lossy()),
        workdir = ps_quote(&install_root.to_string_lossy()),
        icon = ps_quote(&format!("{},0", exe.to_string_lossy()))
    );
    run_powershell(&script)?;
    Ok(true)
}

#[cfg(not(windows))]
fn create_start_menu_shortcut(_install_root: &Path) -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(windows)]
fn register_uninstall_entry(
    install_root: &Path,
    version: &str,
    estimated_size_kb: u64,
) -> Result<bool, EngineError> {
    let exe = install_root.join("Codex.exe");
    let uninstall_script = format!(
        "if ($env:APPDATA) {{ $Shortcut = Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\Codex.lnk'; Remove-Item -LiteralPath $Shortcut -Force -ErrorAction SilentlyContinue }}; Remove-Item -LiteralPath '{}' -Recurse -Force -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Codex' -Recurse -Force -ErrorAction SilentlyContinue",
        install_root.to_string_lossy().replace('\'', "''")
    );
    // Wrap the script in DOUBLE quotes, not single, so Windows' uninstall entry
    // actually RUNS it: `-Command '<script>'` makes PowerShell evaluate the text
    // as one string literal and echo it back; `-Command "<script>"` executes it.
    // The install path sits in single quotes inside, and Windows paths can't
    // contain '"', so the outer double quotes stay unambiguous. -ExecutionPolicy
    // Bypass keeps a restrictive machine policy from blocking the removal.
    let uninstall_string = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"{uninstall_script}\""
    );
    let script = format!(
        r#"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayName -Value 'Codex' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayVersion -Value {version} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name Publisher -Value 'OpenAI' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name InstallLocation -Value {install_root} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayIcon -Value {icon} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name UninstallString -Value {uninstall_string} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name QuietUninstallString -Value {uninstall_string} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -Path $key -Name NoRepair -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -Path $key -Name EstimatedSize -Value {estimated_size_kb} -PropertyType DWord -Force | Out-Null
"#,
        version = ps_quote(version),
        install_root = ps_quote(&install_root.to_string_lossy()),
        icon = ps_quote(&format!("{},0", exe.to_string_lossy())),
        uninstall_string = ps_quote(&uninstall_string),
        estimated_size_kb = estimated_size_kb.min(u32::MAX as u64)
    );
    run_powershell(&script)?;
    Ok(true)
}

#[cfg(not(windows))]
fn register_uninstall_entry(
    _install_root: &Path,
    _version: &str,
    _estimated_size_kb: u64,
) -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(windows)]
fn remove_start_menu_shortcut() -> Result<bool, EngineError> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(false);
    };
    let shortcut = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Codex.lnk");
    if shortcut.exists() {
        fs::remove_file(shortcut).map_err(|e| io_err("remove shortcut", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(not(windows))]
fn remove_start_menu_shortcut() -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(windows)]
fn remove_uninstall_entry() -> Result<bool, EngineError> {
    let script = r#"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
if (Test-Path $key) {
  Remove-Item -Path $key -Recurse -Force
  'removed'
} else {
  'missing'
}
"#;
    Ok(run_powershell(script)?.trim().ends_with("removed"))
}

#[cfg(not(windows))]
fn remove_uninstall_entry() -> Result<bool, EngineError> {
    Ok(false)
}

fn dir_size_kb(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total / 1024
}

fn restore_previous_install(
    install_root: &Path,
    backup: &Path,
    had_previous: bool,
) -> Result<(), EngineError> {
    if install_root.exists() {
        fs::remove_dir_all(install_root)
            .map_err(|e| io_err("remove failed portable install", e))?;
    }
    if had_previous && backup.exists() {
        fs::rename(backup, install_root)
            .map_err(|e| io_err("restore portable rollback backup", e))?;
    }
    Ok(())
}

fn rollback_install_error(
    install_root: &Path,
    backup: &Path,
    had_previous: bool,
    err: EngineError,
) -> EngineError {
    match restore_previous_install(install_root, backup, had_previous) {
        Ok(()) => EngineError::Install(format!("{err}; previous install was restored")),
        Err(rollback_err) => {
            EngineError::Install(format!("{err}; rollback failed: {rollback_err}"))
        }
    }
}

fn health_check_portable_install(install_root: &Path, launch: bool) -> Result<bool, EngineError> {
    let exe = install_root.join("Codex.exe");
    if !exe.is_file() {
        return Err(EngineError::Install(format!(
            "portable health check failed: {} is missing",
            exe.display()
        )));
    }
    if !launch {
        return Ok(false);
    }
    hidden_command(&exe)
        .spawn()
        .map(|_| true)
        .map_err(|e| io_err("portable health check launch", e))
}

pub fn install_portable_from_msix(
    msix_path: &Path,
    install_root: &Path,
    relaunch: bool,
) -> Result<PortableInstallReport, EngineError> {
    let root = install_root.display();
    log::info!("portable install start install_root={root}");
    match install_portable_from_msix_inner(msix_path, install_root, true, relaunch) {
        Ok(report) => {
            let root = &report.install_root;
            log::info!("portable install completed install_root={root}");
            Ok(report)
        }
        Err(err) => {
            log::error!(
                "portable install failed install_root={} error={err}",
                install_root.display()
            );
            Err(err)
        }
    }
}

fn install_portable_from_msix_inner(
    msix_path: &Path,
    install_root: &Path,
    manage_process: bool,
    relaunch: bool,
) -> Result<PortableInstallReport, EngineError> {
    let install_parent = install_root.parent().unwrap_or(install_root);
    fs::create_dir_all(install_parent).map_err(|e| io_err("create install parent", e))?;
    let operation_id = uuid::Uuid::new_v4();
    let work_dir = install_parent
        .join(".codex-app-manager-staging")
        .join(format!("portable-{operation_id}"));
    if work_dir.exists() {
        fs::remove_dir_all(&work_dir).map_err(|e| io_err("clear portable staging", e))?;
    }
    fs::create_dir_all(&work_dir).map_err(|e| io_err("create portable staging", e))?;

    let prepared = prepare_portable_payload(msix_path, &work_dir)?;
    let payload = prepared.payload_dir;
    let backup = install_parent.join(format!("Codex.rollback-{operation_id}"));
    let mut notes = Vec::new();

    if manage_process {
        request_codex_close_for_root(30, install_root)?;
    }

    let had_previous = install_root.exists();
    if had_previous {
        fs::rename(install_root, &backup)
            .map_err(|e| io_err("move current install to rollback", e))?;
    }

    match fs::rename(&payload, install_root) {
        Ok(()) => {}
        Err(err) => {
            let _ = restore_previous_install(install_root, &backup, had_previous);
            return Err(io_err("install portable payload (rolled back)", err));
        }
    }

    let relaunched = match health_check_portable_install(install_root, manage_process && relaunch) {
        Ok(relaunched) => relaunched,
        Err(err) => {
            let _ = fs::remove_dir_all(&work_dir);
            return Err(rollback_install_error(
                install_root,
                &backup,
                had_previous,
                err,
            ));
        }
    };

    let shortcut_created = match create_start_menu_shortcut(install_root) {
        Ok(created) => created,
        Err(err) => {
            notes.push(format!("Start menu shortcut was not created: {err}"));
            false
        }
    };
    let uninstall_entry_created = match register_uninstall_entry(
        install_root,
        &prepared.identity.version,
        dir_size_kb(install_root),
    ) {
        Ok(created) => created,
        Err(err) => {
            notes.push(format!(
                "Apps & Features uninstall entry was not created: {err}"
            ));
            false
        }
    };

    let exe = install_root.join("Codex.exe");
    let mut backup_path = None;
    if had_previous && backup.exists() {
        match fs::remove_dir_all(&backup) {
            Ok(()) => {}
            Err(err) => {
                notes.push(format!(
                    "Portable rollback backup could not be removed after successful install: {err}"
                ));
                backup_path = Some(backup.to_string_lossy().into_owned());
            }
        }
    }

    let _ = fs::remove_dir_all(&work_dir);

    let version = read_codex_app_version_from_install_root(install_root)
        .unwrap_or_else(|| prepared.identity.version.clone());

    Ok(PortableInstallReport {
        success: true,
        install_root: install_root.to_string_lossy().into_owned(),
        executable_path: exe.exists().then(|| exe.to_string_lossy().into_owned()),
        version,
        backup_path,
        shortcut_created,
        uninstall_entry_created,
        relaunched,
        message: "Portable Codex install completed.".to_string(),
        notes,
    })
}

/// Remove the user's Codex data directory (`~/.codex`: sign-in, sessions,
/// config). Returns whether a directory was actually deleted. Shared by the
/// portable and MSIX uninstall paths so both honor the "don't keep my data"
/// choice identically: a missing home directory is recorded as a note (nothing
/// to delete), while an IO failure removing an existing directory propagates.
pub fn purge_codex_user_data(notes: &mut Vec<String>) -> Result<bool, EngineError> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        notes.push("User data purge requested but home directory was not available.".to_string());
        return Ok(false);
    };
    let user_data = PathBuf::from(home).join(".codex");
    if user_data.exists() {
        let path = user_data.display();
        log::warn!("purging Codex user data path={path}");
        fs::remove_dir_all(&user_data).map_err(|e| io_err("purge user data", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn uninstall_portable(
    install_root: &Path,
    purge_user_data: bool,
) -> Result<PortableUninstallReport, EngineError> {
    let path = install_root.display();
    log::info!("portable uninstall start path={path}");
    request_codex_close_for_root(30, install_root)?;

    let removed_files = if install_root.exists() {
        fs::remove_dir_all(install_root).map_err(|e| io_err("remove portable install", e))?;
        true
    } else {
        false
    };

    let mut notes = Vec::new();
    let removed_shortcut = match remove_start_menu_shortcut() {
        Ok(removed) => removed,
        Err(err) => {
            notes.push(format!("Start Menu shortcut cleanup failed: {err}"));
            false
        }
    };
    let removed_uninstall_entry = match remove_uninstall_entry() {
        Ok(removed) => removed,
        Err(err) => {
            notes.push(format!(
                "Apps & Features uninstall entry cleanup failed: {err}"
            ));
            false
        }
    };
    let purged_user_data = if purge_user_data {
        purge_codex_user_data(&mut notes)?
    } else {
        false
    };
    let partial = notes.iter().any(|note| note.contains("cleanup failed"));

    let report = PortableUninstallReport {
        success: true,
        partial,
        install_root: install_root.to_string_lossy().into_owned(),
        removed_files,
        removed_shortcut,
        removed_uninstall_entry,
        purged_user_data,
        message: if partial {
            "Portable Codex uninstall completed with cleanup warnings.".to_string()
        } else {
            "Portable Codex uninstall completed.".to_string()
        },
        notes,
    };
    let path = &report.install_root;
    log::info!("portable uninstall completed path={path}");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn write_fake_msix(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.602.3474.0" ProcessorArchitecture="x64" />
</Package>"#,
        )
        .unwrap();
        zip.start_file("VFS/ProgramFilesX64/Codex/Codex.exe", opts)
            .unwrap();
        zip.write_all(b"fake exe").unwrap();
        zip.start_file("VFS/ProgramFilesX64/Codex/resources/app.asar", opts)
            .unwrap();
        zip.write_all(b"fake asar").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn installs_portable_payload_from_msix_layout() {
        let root = std::env::temp_dir().join(format!("codex-portable-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(install_root.join("Codex.exe").exists());
        assert!(install_root.join("resources/app.asar").exists());
        assert!(install_root.join("AppxManifest.xml").exists());
        assert_eq!(report.version, "26.602.3474.0");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn replaces_existing_portable_and_removes_rollback_backup() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-replace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);

        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("Codex.exe"), b"old exe").unwrap();
        fs::write(install_root.join("old-marker.txt"), b"old").unwrap();

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("Codex.rollback")));
        assert!(!install_root.join("old-marker.txt").exists());
        assert!(install_root.join("resources/app.asar").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_previous_install_removes_failed_payload() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-rollback-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let install_root = root.join("Codex");
        let backup = root.join("Codex.rollback-test");

        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("old-marker.txt"), b"old").unwrap();
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("new-marker.txt"), b"new").unwrap();

        restore_previous_install(&install_root, &backup, true).unwrap();
        assert!(install_root.join("old-marker.txt").exists());
        assert!(!install_root.join("new-marker.txt").exists());
        assert!(!backup.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
