# Mirror Manifest Contract

The manager expects `codex-app-mirror` to keep a stable manifest at:

```text
https://codexapp.agentsmirror.com/latest/manifest
```

Minimum data needed by the manager:

```json
{
  "schemaVersion": 2,
  "sources": {
    "windows": {
      "version": "26.602.3474.0",
      "packageMoniker": "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
      "contentLength": 0,
      "etag": "",
      "architectures": {
        "x64": {
          "architecture": "x64",
          "status": "downloadable",
          "downloadable": true,
          "version": "26.602.3474.0",
          "packageMoniker": "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
          "contentLength": 0
        },
        "arm64": {
          "architecture": "arm64",
          "status": "downloadable",
          "downloadable": true,
          "version": "26.602.3474.0",
          "packageMoniker": "OpenAI.Codex_26.602.3474.0_arm64__2p2nqsd0c76g0",
          "contentLength": 0
        }
      },
      "updateManifest": {
        "packageIdentity": "OpenAI.Codex",
        "storeProductId": "9PLM9XGG6VKS"
      }
    },
    "macos": {
      "arm64": {
        "bundleShortVersion": "26.602.30954",
        "bundleVersion": "3575",
        "sha256": ""
      },
      "x64": {
        "bundleShortVersion": "26.602.30954",
        "bundleVersion": "3575",
        "sha256": ""
      }
    }
  },
  "derived": {
    "windowsVersion": "26.602.3474.0"
  }
}
```

For Windows, the manager prefers `sources.windows.architectures.<arch>` when it is
present and `downloadable` is not `false`. ARM64 Windows uses
`https://codexapp.agentsmirror.com/latest/win-arm64`; x64 Windows uses
`https://codexapp.agentsmirror.com/latest/win-x64`. The older top-level
`packageMoniker` and `/latest/win` route remain compatibility fallbacks for older
mirror manifests or custom sources.

Future manager-specific fields can live under:

```json
{
  "manager": {
    "payloads": {
      "windowsX64Msix": {
        "url": "https://codexapp.agentsmirror.com/latest/win-x64",
        "sha256": ""
      },
      "windowsArm64Msix": {
        "url": "https://codexapp.agentsmirror.com/latest/win-arm64",
        "sha256": ""
      },
      "macosArm64Dmg": {
        "url": "https://codexapp.agentsmirror.com/latest/mac-arm64",
        "sha256": ""
      },
      "macosIntelDmg": {
        "url": "https://codexapp.agentsmirror.com/latest/mac-intel",
        "sha256": ""
      }
    }
  }
}
```
