export default {
  meta: {
    title: "Codex App Manager — install, update & uninstall the official Codex desktop app",
    description:
      "One-click install, Sparkle delta updates on macOS, and clean uninstall for the official OpenAI Codex desktop app. Verified byte by byte, mirrored verbatim, and reachable from mainland China without a proxy. Open source, MIT licensed.",
    ogTitle: "Codex App Manager — official Codex, installed in one click",
    ogDescription:
      "Install, update, and uninstall the official Codex desktop app — macOS delta updates with automatic rollback, verbatim mirrors reachable from mainland China. Signed, notarized, open source.",
  },
  nav: {
    why: "Why",
    manager: "The Manager",
    pipeline: "The Pipeline",
    trust: "Trust",
    download: "Download",
    scope: "Scope",
    cta: "Download",
  },
  hero: {
    eyebrow: "Open source · macOS & Windows",
    titleA: "The official Codex app.",
    titleB: "One click. Always current.",
    tagline:
      "One-click install, delta updates, and clean uninstall for the official OpenAI Codex desktop app — with self-updates that reach mainland China.",
    sub: "No Microsoft Store needed on Windows. On macOS, updates download only what changed, verified byte by byte. Dual mirrors — R2 globally, IHEP in mainland China — no proxy required.",
    ctaPrimary: "Download now",
    ctaSecondary: "View on GitHub",
    allOptions: "All download options",
    dl: {
      macArm: "Download for Apple Silicon",
      macIntel: "Download for Intel Mac",
      win: "Download for Windows (x64)",
    },
    scrollHint: "See how it works",
    badges: ["Signed & notarized", "MIT licensed", "Tauri v2", "11 languages"],
  },
  demo: {
    window: "Codex App Manager",
    updTitle: "Update available",
    updVer: "26.602.71036",
    updFlow: "Now 26.602.40724 → New 26.602.71036 · ~12.6 MiB",
    updCta: "Update now",
    launch: "Launch Codex",
    recheck: "Check again",
    checking: "Checking…",
    progressTitle: "Updating…",
    progressFrom: "Downloading from Mirror",
    uptodateTitle: "Up to date",
    uptodateSub: "Current version 26.602.71036",
    official: "Official build · Just checked",
    successFlow: "Updated 26.602.40724 → 26.602.71036",
    releaseDate: "Released",
    releaseVal: "Today",
    loc: "Location",
    locVal: "/Applications/Codex.app",
    source: "Source: Mirror",
    settings: "Settings",
    managed: "Managed",
  },
  pain: {
    kicker: "The problem",
    title: "Keeping Codex current shouldn't be a chore",
    lead: "The official Codex desktop app is great. Getting it onto your machine — and keeping it there, current and intact — depends on a download chain that wasn't built for everyone.",
    cards: [
      {
        title: "The Microsoft Store problem",
        body: "On Windows, the official Codex ships only through the Microsoft Store. Store unreachable, account won't sign in, Store disabled by policy — any one of these and you can't install Codex at all.",
      },
      {
        title: "Hosted an ocean away",
        body: "The official macOS download and the Sparkle update chain both live on overseas infrastructure. One flaky connection and downloads retry, updates stall, and you quietly fall behind.",
      },
      {
        title: "Manual installs are a gamble",
        body: "Dragging DMGs around by hand means no verification, no rollback, and stray files from versions past. One bad update and you're reinstalling from scratch.",
      },
    ],
    mock: {
      file: "Codex_Installer.dmg",
      host: "Source: overseas node",
      status: "Network timeout, retrying…",
      retry: "Attempt 3",
    },
  },
  manager: {
    kicker: "The Manager",
    title: "Install, update, uninstall — without the mess",
    lead: "Codex App Manager treats every operation as three honest steps. It looks before it leaps, and it verifies before doing anything destructive.",
    steps: [
      {
        name: "Detect",
        title: "Know the machine first",
        body: "The Manager scans your local Codex install — version, location, state — before it touches anything. No assumptions, no blind writes.",
      },
      {
        name: "Plan",
        title: "Plan the exact change",
        body: "From that state it computes a precise plan: what gets downloaded, what gets replaced, what gets removed. Destructive steps are verified before they run.",
      },
      {
        name: "Execute",
        title: "Execute, then verify",
        body: "The plan runs and every step is checked. If an update fails verification on macOS, it rolls back automatically — and when it's done, Codex is one click from launching.",
      },
    ],
    scenarios: [
      {
        label: "Install",
        desc: "From nothing to a running Codex in one click. No DMG dragging, no setup-wizard archaeology.",
      },
      {
        label: "Update",
        desc: "On macOS, the Manager reads the Sparkle appcast and downloads only the delta between your version and the latest — EdDSA signature verified, byte by byte.",
      },
      {
        label: "Rollback",
        desc: "If a delta fails verification or an update goes sideways, the previous version comes back automatically. You're never stranded on a broken install.",
      },
    ],
    extras: [
      {
        title: "One-click launch",
        body: "When the work is done, launch Codex straight from the Manager.",
      },
      {
        title: "No Microsoft Store needed",
        body: "Installs the official MSIX or portable build directly, stages updates, and runs a post-install health check — even when the Store is out of reach.",
      },
      {
        title: "A UI worth keeping open",
        body: "Warm OKLCH material design, dark and light themes, GSAP motion — in 11 languages, including Arabic with full RTL.",
      },
    ],
    mock: {
      window: "Codex App Manager",
      detect: {
        scan: "Scanning this machine…",
        found: "Codex desktop app found",
        ver: "Version behind upstream",
        leftover: "Old version leftovers detected",
      },
      plan: {
        title: "Execution plan",
        i1: "Download delta package",
        i2: "Verify EdDSA signature",
        i3: "Replace application files",
        i4: "Run health check",
        note: "Destructive steps are verified first",
      },
      exec: {
        doing: "Applying update…",
        ok: "Updated · signature verified",
        launch: "Launch Codex",
        rollback: "Rolls back automatically if verification fails",
      },
    },
  },
  pipeline: {
    kicker: "The Mirror",
    title: "From the official upstream to your desktop",
    lead: "Codex App Mirror watches the official upstream and redistributes it verbatim — verifiably, and reachable from mainland China. The Manager turns that pipeline into the install and update experience on your desktop. Here's the full route an installer travels.",
    stages: [
      {
        title: "It starts upstream",
        body: "Every journey begins with an official OpenAI release: Windows MSIX and macOS DMG, arm64 and x64. These are the bytes — exactly as shipped.",
        stat: "MSIX + DMG",
      },
      {
        title: "Probed every 15 minutes",
        body: "A Cloudflare Cron checks the upstream every 15 minutes, with GitHub Actions standing by as fallback. The moment something changes, the pipeline wakes up.",
        stat: "15 min",
      },
      {
        title: "Released, untouched",
        body: "A new mirror release goes out automatically — zero modification, zero repackaging. Every release ships SHA256SUMS and a fingerprint manifest of the upstream it came from.",
        stat: "SHA256",
      },
      {
        title: "Landed on two nodes",
        body: "The bytes settle on two nodes: Cloudflare R2 for the world, IHEP S3 for mainland China. If one path is slow, the other isn't.",
        stat: "R2 + IHEP",
      },
      {
        title: "One link, best node",
        body: "A Cloudflare Worker routes by CF-IPCountry: requests from mainland China get IHEP presigned URLs, everyone else gets R2. You never have to choose.",
        stat: "1 URL",
      },
      {
        title: "Applied as a delta",
        body: "On macOS, the Manager reads the Sparkle appcast, pulls only the delta between versions, and verifies the official EdDSA signature byte by byte. If anything is off, it rolls back.",
        stat: "EdDSA",
      },
    ],
    nodes: ["Upstream", "15-min probe", "Auto release", "Dual mirrors", "Geo routing", "Your desktop"],
    branchGlobal: "Global → R2",
    branchCN: "Mainland China → IHEP S3",
    verifiedChip: "EdDSA verified",
    finale: "From the official upstream to your Mac or PC — every byte arrives intact.",
  },
  trust: {
    kicker: "Trust",
    title: "Verify every hop yourself",
    lead: "Nothing here asks for your trust. Whether the mirror matches the official release is something anyone can check, any time.",
    mock: {
      title: "Checksum comparison",
      tag: "Schematic",
      official: "Official SHA256",
      mirror: "Mirror SHA256",
      sig: "EdDSA signature",
      sigVal: "Copied byte-for-byte from upstream",
      match: "Exact match",
    },
    items: [
      {
        title: "Developer ID signed, Apple notarized",
        body: "Every macOS build of the Manager is signed with a Developer ID certificate and notarized by Apple. Gatekeeper opens it without a fight.",
      },
      {
        title: "EdDSA, byte for byte",
        body: "The mirror copies the official Sparkle EdDSA signatures exactly as published — it doesn't forge them, and it couldn't. On macOS, the Manager verifies each delta against those signatures before applying it.",
      },
      {
        title: "Checksums and fingerprints",
        body: "Every mirror release ships SHA256SUMS plus a manifest fingerprinting the upstream it was taken from. Diff it against the official release yourself — that's the point.",
      },
      {
        title: "Open source, MIT",
        body: "The Manager, the mirror pipeline, the routing worker — all of it is public code under the MIT license. Don't take our word for any of this; read it.",
      },
    ],
  },
  download: {
    kicker: "Download",
    title: "Download Codex App Manager",
    lead: "Pick your platform. Every link below resolves to the latest release and works from mainland China without a proxy.",
    recommended: "Recommended for you",
    brew: {
      title: "Homebrew",
      note: "macOS recommended",
    },
    direct: [
      {
        platform: "macOS · Apple Silicon",
        label: "Download .dmg",
        note: "Always-latest permalink · reachable from mainland China",
      },
      {
        platform: "macOS · Intel",
        label: "Download .dmg",
        note: "Always-latest permalink · reachable from mainland China",
      },
      {
        platform: "Windows · x64",
        label: "Download .exe",
        note: "Always-latest permalink · no Microsoft Store needed",
      },
    ],
    github: {
      title: "On GitHub",
      note: "Source, issues, and the full pipeline — audit away.",
      managerLabel: "Wangnov/Codex-App-Manager",
      mirrorLabel: "Wangnov/codex-app-mirror",
    },
  },
  scope: {
    kicker: "Scope",
    title: "What this project is — and isn't",
    items: [
      {
        title: "Verbatim or nothing",
        body: "The mirror never modifies or repackages official installers, and it never forges signatures. EdDSA signatures are copied byte for byte from upstream — that is the only way they can exist.",
      },
      {
        title: "No affiliation",
        body: "This is an independent, community-built project. It is not affiliated with, nor endorsed by, OpenAI or Microsoft.",
      },
      {
        title: "Open and auditable",
        body: "Both projects are MIT licensed and fully open source. Every claim on this page can be checked against the code.",
      },
    ],
    mit: "MIT licensed — use it, fork it, audit it.",
  },
  footer: {
    thanks:
      "With thanks to the Institute of High Energy Physics, Chinese Academy of Sciences (IHEP) for hosting the mainland-China mirror node.",
    license: "MIT License",
    made: "Built with Tauri v2 and an unreasonable concern for bytes.",
    backTop: "Back to top",
    links: {
      manager: "Codex App Manager",
      mirror: "Codex App Mirror",
    },
  },
  ui: {
    langSwitch: "中文",
    langAria: "Switch language",
    menu: "Menu",
    close: "Close",
    skip: "Skip to content",
    copy: "Copy",
    copied: "Copied",
  },
} as const;
