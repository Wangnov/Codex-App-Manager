import type { Platform } from "./platform";

export function codexHomeDisplay(platform: Platform): string {
  return platform === "windows" ? "%USERPROFILE%\\.codex" : "~/.codex";
}
