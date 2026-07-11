# Fail-closed release placeholder for the SignPath Foundation migration.
#
# This script intentionally has no success path. Remove or replace it only in a
# reviewed change that implements an approved SignPath trusted-build flow,
# manual per-release approval, project-owned artifact boundaries, and final
# Authenticode/timestamp verification. Setting a secret or variable must never
# bypass this guard by itself.

[CmdletBinding()]
param(
    [string]$Stage = "signpath-readiness"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$message = @(
    "Windows release signing is blocked: the SignPath Foundation application and trusted-build migration are not complete."
    "No certificate has been issued and no production signing integration is active."
    "See docs/code-signing-policy.md and docs/windows-signing.md."
) -join " "

Write-Host "::error title=Windows release is fail-closed::[$Stage] $message"
throw "[$Stage] $message"
