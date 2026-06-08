// Shared display formatters used by the platform home views.

/** Bytes → "12.3 MB" (binary mebibytes, one decimal). */
export function mib(bytes: number): string {
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

/** Localized date + time, in the user's own timezone, for a Sparkle pubDate
 *  (RFC-822 string) or a Unix timestamp in seconds (bundle/install mtime).
 *  Returns null when absent/unparseable so the row is simply omitted. */
export function fmtDateTime(
  value: string | number | null | undefined,
  lang: string,
): string | null {
  if (value === null || value === undefined || value === "") return null;
  const d = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(d.getTime())) return null;
  try {
    // dateStyle/timeStyle = compact + fully localized; the runtime's local
    // timezone is used automatically (no timeZone option = system time).
    return new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "short" }).format(d);
  } catch {
    return d.toLocaleString();
  }
}
