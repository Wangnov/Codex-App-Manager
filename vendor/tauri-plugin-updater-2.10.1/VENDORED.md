# Vendored tauri-plugin-updater 2.10.1

This directory is an exact source-vendor of
[`tauri-plugin-updater` 2.10.1](https://crates.io/crates/tauri-plugin-updater/2.10.1),
whose crates.io checksum is
`806d9dac662c2e4594ff03c647a552f2c9bd544e7d0f683ec58f872f952ce4af`.
The upstream repository is
[`tauri-apps/plugins-workspace`](https://github.com/tauri-apps/plugins-workspace).
The original Apache-2.0 and MIT license files are preserved beside this note.

## Local security patch

Upstream 2.10.1 reads both `latest.json` and updater artifacts completely into
memory. `configure_client` only exposes `reqwest::ClientBuilder`, so callers
cannot impose a response-body limit. This vendor adds:

- explicit non-zero default manifest and artifact byte limits;
- `UpdaterBuilder::max_manifest_size` and `max_download_size` overrides;
- `Content-Length` preflight plus checked streaming byte accumulation;
- an explicit `ResponseTooLarge` error before an oversized chunk is retained;
- the selected static-manifest platform key on `Update`;
- public reuse of the plugin's exact minisign verification routine for signed
  release identity files;
- URL-free request, response-stream, and manifest-schema diagnostics, including
  fixed proxy and endpoint debug messages so untrusted strings or presigned URL
  credentials cannot enter logs; and
- regression tests for header-declared and chunked/no-header oversized bodies.

No installer, extraction, proxy, TLS, or minisign artifact semantics are
otherwise changed.

The standalone `Cargo.lock` is retained so CI can run this vendor's own tests
with `--locked`; the application still resolves the path patch through
`src-tauri/Cargo.lock`.

## Upgrade checklist

Do not remove the `[patch.crates-io]` entry during an updater upgrade until the
candidate upstream version is verified to provide all of these properties:

1. Manifest and artifact responses have both header and streamed hard limits.
2. The limit aborts before appending the first over-limit chunk.
3. Minisign verification still runs before `Update::install`.
4. Codex App Manager can identify the exact selected platform and verify its
   signed release identity before trusting a mirror manifest.
5. The tests in this vendor and the Manager replay/fallback tests have been
   ported and pass against the replacement.
6. Manifest and artifact request/stream errors discard associated URLs before
   logging or display while retaining transport category and HTTP status, and
   malformed manifest schemas never echo attacker-controlled string values.

When upgrading, diff this directory against the exact new crate source, retain
the upstream license files, update the checksum/version above, regenerate
`src-tauri/Cargo.lock`, and run both the vendor and application Rust suites.
