import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/sections.css";
import "./styles/pipeline.css";

import gsap from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import { MotionPathPlugin } from "gsap/MotionPathPlugin";
import { applyLang, initialLang, t, type Lang } from "./i18n";

gsap.registerPlugin(ScrollTrigger, MotionPathPlugin);

/* ============================== i18n ==================================== */

let lang: Lang = initialLang();

function syncLangUI() {
  const label = document.getElementById("lang-switch-label");
  if (label) label.textContent = t(lang, "ui.langSwitch") as string;
}

applyLang(lang);
syncLangUI();

/* ====================== loading orchestration =========================== */

// The shell fades once the critical pixels are in: hero image + display font,
// capped so a slow connection never stares at the spinner.
const ready: Promise<void> = (() => {
  const heroImg = document.querySelector<HTMLImageElement>(".hero-bg img");
  const imgReady: Promise<void> =
    heroImg && !heroImg.complete
      ? new Promise((r) => {
          heroImg.addEventListener("load", () => r(), { once: true });
          heroImg.addEventListener("error", () => r(), { once: true });
        })
      : Promise.resolve();
  const fontsReady: Promise<unknown> = document.fonts?.ready ?? Promise.resolve();
  const cap = new Promise<void>((r) => setTimeout(r, 2200));
  return Promise.race([Promise.all([imgReady, fontsReady]).then(() => undefined), cap]);
})();

ready.then(() => {
  document.body.classList.add("loaded");
  window.setTimeout(() => document.getElementById("preloader")?.remove(), 600);
});

// Every image stays invisible until decoded, then eases in — no half-painted
// decorative layers while the network catches up.
document.querySelectorAll<HTMLImageElement>("img").forEach((img) => {
  if (img.closest(".preloader")) return;
  img.classList.add("fade-in");
  if (img.complete && img.naturalWidth > 0) return;
  img.classList.add("fade-pending");
  const done = () => img.classList.remove("fade-pending");
  img.addEventListener("load", done, { once: true });
  img.addEventListener("error", done, { once: true });
});

document.getElementById("lang-switch")?.addEventListener("click", () => {
  lang = lang === "zh" ? "en" : "zh";
  applyLang(lang);
  syncLangUI();
  // headline lengths differ a lot between languages
  requestAnimationFrame(() => ScrollTrigger.refresh());
});

/* ============================== nav ===================================== */

const nav = document.getElementById("nav")!;
const onScroll = () => nav.classList.toggle("is-scrolled", window.scrollY > 24);
onScroll();
addEventListener("scroll", onScroll, { passive: true });

const burger = document.getElementById("nav-burger");
const menu = document.getElementById("nav-menu");

function setMenu(open: boolean) {
  document.body.classList.toggle("menu-open", open);
  burger?.setAttribute("aria-expanded", String(open));
  burger?.setAttribute("aria-label", t(lang, open ? "ui.close" : "ui.menu") as string);
  menu?.setAttribute("aria-hidden", String(!open));
}

burger?.addEventListener("click", () =>
  setMenu(!document.body.classList.contains("menu-open"))
);
menu?.querySelectorAll("a").forEach((a) =>
  a.addEventListener("click", () => setMenu(false))
);

/* ===================== decorative hex stream (trust) ==================== */

(() => {
  const host = document.getElementById("trust-hex");
  if (!host) return;
  // deterministic LCG so the texture is stable between loads
  let seed = 0x5f3a;
  const rnd = () => ((seed = (seed * 48271) % 0x7fffffff) & 0xff)
    .toString(16)
    .padStart(2, "0");
  const lines: string[] = [];
  for (let i = 0; i < 26; i++) {
    lines.push(Array.from({ length: 48 }, rnd).join(" "));
  }
  host.textContent = lines.join("\n");
})();

/* ====================== platform recommendation ========================= */

(() => {
  const ua = navigator.userAgent;
  let platform: string | null = null;
  // iPhone/iPad UAs contain "like Mac OS X", and desktop-mode iPadOS even
  // reports "Macintosh" — neither can run a DMG, so bail out before the Mac
  // branch (touch points are the reliable tell for masquerading iPads).
  const isAppleMobile =
    /iPhone|iPad|iPod/i.test(ua) ||
    (/Macintosh/i.test(ua) && navigator.maxTouchPoints > 1);
  if (/Windows/i.test(ua)) platform = "windows";
  else if (!isAppleMobile && /Macintosh/i.test(ua)) {
    // Only commit to an architecture on a positive GPU signal — guessing
    // wrong would deep-link the hero CTA to an installer that won't run.
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      const dbg = gl?.getExtension("WEBGL_debug_renderer_info");
      const renderer = dbg
        ? String(gl!.getParameter(dbg.UNMASKED_RENDERER_WEBGL))
        : "";
      // Order matters: Chrome's ANGLE-on-Metal renderer string on Intel Macs
      // reads "ANGLE (Apple, ... Intel ...)" — the discrete-GPU vendors are
      // the discriminating signal, "Apple" alone is not.
      if (/(intel|amd|nvidia|radeon)/i.test(renderer)) platform = "mac-intel";
      else if (/apple/i.test(renderer)) platform = "mac-arm";
    } catch {
      /* unknown — let the user pick from the download section */
    }
  }
  if (!platform) return;
  document
    .querySelector(`.dl-card[data-platform="${platform}"]`)
    ?.classList.add("is-recommended");

  // the hero CTA deep-links straight to the matching installer
  const cta = document.getElementById("hero-dl") as HTMLAnchorElement | null;
  const targets: Record<string, { href: string; key: string }> = {
    "mac-arm": {
      href: "https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_aarch64.dmg",
      key: "hero.dl.macArm",
    },
    "mac-intel": {
      href: "https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x86_64.dmg",
      key: "hero.dl.macIntel",
    },
    windows: {
      href: "https://codexapp.agentsmirror.com/manager/latest/CodexAppManager_x64-setup.exe",
      key: "hero.dl.win",
    },
  };
  const entry = targets[platform];
  if (cta && entry) {
    cta.href = entry.href;
    cta.dataset.i18n = entry.key; // applyLang keeps the label right after a switch
    cta.textContent = t(lang, entry.key) as string;
  }
})();

/* ===================== interactive product demo ========================= */

/* keep the 400x640 replica at the right scale inside the laptop screen */
(() => {
  const screen = document.getElementById("mac-screen");
  const win = document.getElementById("cam-demo");
  if (!screen || !win) return;
  const fit = () => {
    if (window.innerWidth < 1024) return; // phones show the bare window
    const scale = Math.min((screen.clientHeight * 0.92) / 640, (screen.clientWidth * 0.86) / 400);
    win.style.setProperty("--app-scale", scale.toFixed(4));
  };
  fit();
  addEventListener("resize", fit, { passive: true });
})();

(() => {
  const win = document.getElementById("cam-demo");
  if (!win) return;
  const scenes = win.querySelectorAll<HTMLElement>(".cam-scene");
  const banner = document.getElementById("cam-banner")!;
  const pct = document.getElementById("cam-pct")!;
  const fill = document.getElementById("cam-bar-fill")!;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  let bannerTimer = 0;
  let busy = false;

  const show = (name: string) =>
    scenes.forEach((sc) => sc.classList.toggle("is-active", sc.dataset.scene === name));

  const runUpdate = () => {
    if (busy) return;
    busy = true;
    banner.classList.remove("is-show");
    show("progress");
    const state = { p: 0 };
    gsap.to(state, {
      p: 100,
      duration: reduced ? 0 : 2.2,
      ease: "power1.inOut",
      onUpdate() {
        pct.textContent = String(Math.round(state.p));
        fill.style.width = `${state.p}%`;
      },
      onComplete() {
        show("done");
        banner.classList.add("is-show");
        clearTimeout(bannerTimer);
        bannerTimer = window.setTimeout(() => banner.classList.remove("is-show"), 4200);
        busy = false;
      },
    });
  };

  const runRecheck = () => {
    if (busy) return;
    busy = true;
    show("checking");
    window.setTimeout(
      () => {
        show("done");
        busy = false;
      },
      reduced ? 60 : 1100
    );
  };

  win.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("[data-demo]");
    if (!btn) return;
    if (btn.dataset.demo === "update") runUpdate();
    else if (btn.dataset.demo === "recheck") runRecheck();
    else gsap.fromTo(btn, { scale: 1 }, { scale: 0.94, yoyo: true, repeat: 1, duration: 0.12 });
  });
})();

/* ============================ copy button =============================== */

(() => {
  const btn = document.getElementById("copy-brew");
  const cmd = document.getElementById("brew-cmd");
  if (!btn || !cmd) return;
  let timer = 0;
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(cmd.textContent ?? "");
      btn.classList.add("is-copied");
      btn.textContent = t(lang, "ui.copied") as string;
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        btn.classList.remove("is-copied");
        btn.textContent = t(lang, "ui.copy") as string;
      }, 1800);
    } catch {
      /* clipboard unavailable */
    }
  });
})();

/* ============================== motion ================================== */

const mm = gsap.matchMedia();

/* manager-enactment lookups — declared before any mm.add(), because a
   matching context (e.g. prefers-reduced-motion) runs its callback
   synchronously at registration */
const steps = gsap.utils.toArray<HTMLElement>("#manager-steps .step");
const panels = gsap.utils.toArray<HTMLElement>(".mock-panel");

function setManagerStage(k: number) {
  steps.forEach((s, i) => s.classList.toggle("is-active", i === k));
  panels.forEach((p, i) => p.classList.toggle("is-active", i === k));
}

mm.add("(prefers-reduced-motion: no-preference)", () => {
  /* ---- generic reveals (hero has its own intro) ---- */
  document.querySelectorAll<HTMLElement>("[data-reveal]").forEach((el) => {
    if (el.closest(".hero")) return;
    gsap.from(el, {
      y: 30,
      autoAlpha: 0,
      duration: 1,
      ease: "power3.out",
      scrollTrigger: { trigger: el, start: "top 86%", once: true },
    });
  });
  document.querySelectorAll<HTMLElement>("[data-reveal-group]").forEach((group) => {
    gsap.from(group.children, {
      y: 34,
      autoAlpha: 0,
      duration: 0.9,
      ease: "power3.out",
      stagger: 0.12,
      scrollTrigger: { trigger: group, start: "top 84%", once: true },
    });
  });

  /* ---- hero intro (waits for the preloader to lift) ---- */
  void ready.then(() => {
    gsap.from(".hero-title .line > span", {
      yPercent: 118,
      duration: 1.25,
      ease: "power4.out",
      stagger: 0.14,
      delay: 0.1,
    });
    gsap.from(".hero-copy [data-reveal]", {
      y: 26,
      autoAlpha: 0,
      duration: 1,
      ease: "power3.out",
      stagger: 0.1,
      delay: 0.4,
    });
    gsap.from(".hero-cloud", {
      y: 60,
      autoAlpha: 0,
      duration: 1.6,
      ease: "power3.out",
      delay: 0.25,
    });
  });
  gsap.from(".hero-demo", {
    y: 60,
    autoAlpha: 0,
    duration: 1.1,
    ease: "power3.out",
    scrollTrigger: { trigger: ".hero-demo", start: "top 88%", once: true },
  });

  /* ---- hero pointer parallax (fine pointers only) ---- */
  let onPointer: ((e: Event) => void) | undefined;
  const hero = document.querySelector(".hero");
  if (matchMedia("(pointer: fine)").matches && hero) {
    const layers = Array.from(
      document.querySelectorAll<HTMLElement>(".hero [data-depth], .hero img.layer")
    );
    const movers = layers.map((el) => ({
      depth: parseFloat(el.dataset.depth ?? "0.15"),
      x: gsap.quickTo(el, "x", { duration: 0.9, ease: "power3.out" }),
      y: gsap.quickTo(el, "y", { duration: 0.9, ease: "power3.out" }),
    }));
    onPointer = (e) => {
      const { innerWidth: w, innerHeight: h } = window;
      const nx = (e as PointerEvent).clientX / w - 0.5;
      const ny = (e as PointerEvent).clientY / h - 0.5;
      movers.forEach((m) => {
        m.x(nx * 70 * m.depth);
        m.y(ny * 46 * m.depth);
      });
    };
    hero.addEventListener("pointermove", onPointer);
  }

  /* ---- checksum card: hashes scramble, settle identical, chip pops ---- */
  const hashes = gsap.utils.toArray<HTMLElement>("[data-vc-hash]");
  if (hashes.length) {
    const finals = hashes.map((h) => h.textContent ?? "");
    ScrollTrigger.create({
      trigger: ".verify-card",
      start: "top 78%",
      once: true,
      onEnter: () => {
        const HEX = "0123456789abcdef";
        const state = { p: 0 };
        gsap.to(state, {
          p: 1,
          duration: 1.1,
          ease: "power2.out",
          onUpdate() {
            hashes.forEach((h, i) => {
              const final = finals[i];
              h.textContent = final
                .split("")
                .map((ch, j) =>
                  ch === " " || j / final.length < state.p
                    ? ch
                    : HEX[(Math.random() * 16) | 0]
                )
                .join("");
            });
          },
          onComplete() {
            hashes.forEach((h, i) => (h.textContent = finals[i]));
          },
        });
        gsap.from("#vc-match-chip", {
          scale: 0.5,
          autoAlpha: 0,
          duration: 0.5,
          ease: "back.out(2.2)",
          delay: 1.05,
        });
      },
    });
  }

  return () => {
    if (onPointer) hero?.removeEventListener("pointermove", onPointer);
  };
});

/* ---- reduced motion: settle everything into its final, visible state ---- */
mm.add("(prefers-reduced-motion: reduce)", () => {
  gsap.set("#rail-fill", { scaleY: 1 });
  document.querySelectorAll(".rail-item").forEach((el) => el.classList.add("is-lit"));
  // show the final "execute" panel, with every step lit and the outcome settled
  setManagerStage(2);
  steps.forEach((s) => s.classList.add("is-active"));
  gsap.set("#mock-progress-bar", { width: "100%" });
  gsap.set("#mock-done, #mock-launch", { opacity: 1 });
  return () => {};
});

/* ---- manager enactment: desktop = pinned scrub, mobile = step triggers -- */

mm.add(
  "(min-width: 1024px) and (prefers-reduced-motion: no-preference)",
  () => {
    /* hero scroll parallax — desktop only: on phones the hero is taller than
       the viewport, so fading the content out would hide the demo mid-read */
    const heroTl = gsap
      .timeline({
        scrollTrigger: {
          trigger: ".hero",
          start: "top top",
          end: "bottom top",
          scrub: true,
        },
      })
      .to(".hero-bg img", { yPercent: 14, scale: 1.06, ease: "none" }, 0)
      .to(".hero-bokeh", { yPercent: -16, ease: "none" }, 0)
      .to(".hero-mist", { yPercent: -26, ease: "none" }, 0)
      .to(".hero-cloud", { yPercent: -52, rotation: 2.5, ease: "none" }, 0);

    /* pinned manager */
    const checks = gsap.utils.toArray<HTMLElement>(".mock-panel .mock-check");
    const tl = gsap.timeline({
      scrollTrigger: {
        trigger: "#manager-stage",
        start: "top 12%",
        end: "+=2200",
        pin: true,
        scrub: 0.6,
        onUpdate(self) {
          const p = self.progress;
          const k = p < 0.33 ? 0 : p < 0.66 ? 1 : 2;
          setManagerStage(k);
          checks.forEach((c, i) =>
            c.classList.toggle(
              "is-done",
              p > 0.38 + i * 0.062 // ticks march down the plan list
            )
          );
        },
      },
    });
    tl.fromTo(
      ".mock-scanline",
      { y: -30, opacity: 0.9 },
      { y: 360, opacity: 0.4, duration: 30, ease: "none" },
      0
    )
      .to("#mock-progress-bar", { width: "100%", duration: 22, ease: "none" }, 70)
      .to("#mock-done", { opacity: 1, duration: 4 }, 92)
      .to("#mock-launch", { opacity: 1, duration: 4 }, 95);

    /* pinned pipeline */
    const pipeTl = buildPipeline();

    return () => {
      pipeTl?.kill();
      setManagerStage(0);
    };
  }
);

mm.add("(max-width: 1023px) and (prefers-reduced-motion: no-preference)", () => {
  /* manager steps activate as they pass; exec panel plays a one-shot fill */
  const triggers = steps.map((step, i) =>
    ScrollTrigger.create({
      trigger: step,
      start: "top 62%",
      onEnter: () => {
        setManagerStage(i);
        if (i === 2) {
          gsap.to("#mock-progress-bar", { width: "100%", duration: 1.1, ease: "power1.inOut" });
          gsap.to("#mock-done, #mock-launch", { opacity: 1, delay: 1.0, duration: 0.5 });
        }
      },
      onEnterBack: () => setManagerStage(i),
    })
  );

  /* pipeline rail */
  const fill = document.getElementById("rail-fill");
  let railTl: gsap.core.Tween | undefined;
  if (fill) {
    railTl = gsap.to(fill, {
      scaleY: 1,
      ease: "none",
      scrollTrigger: {
        trigger: "#pipeline-rail",
        start: "top 64%",
        end: "bottom 78%",
        scrub: true,
      },
    });
  }
  const items = gsap.utils.toArray<HTMLElement>(".rail-item");
  const itemTriggers = items.map((item) =>
    ScrollTrigger.create({
      trigger: item,
      start: "top 68%",
      onEnter: () => item.classList.add("is-lit"),
      onLeaveBack: () => item.classList.remove("is-lit"),
    })
  );

  return () => {
    triggers.forEach((tr) => tr.kill());
    itemTriggers.forEach((tr) => tr.kill());
    railTl?.kill();
  };
});

/* ===================== the pipeline (pinned, scrubbed) =================== */

function buildPipeline(): gsap.core.Timeline | undefined {
  const stage = document.getElementById("pipeline-stage");
  if (!stage) return;

  const cards = gsap.utils.toArray<HTMLElement>(".stage-card");
  const nodes = [
    "#node-0",
    "#node-1",
    "#node-2",
    "#node-r2",
    "#node-4",
    "#node-5",
  ];
  const labels = gsap.utils.toArray<SVGTextElement>(".pipe-label");
  const bars = gsap.utils.toArray<HTMLElement>("#stage-progress i");
  const index = document.getElementById("stage-index")!;

  // prepare lit paths for progressive draw
  const litPairs = [
    ["#lit-1", "#lit-glow-1"],
    ["#lit-r2", "#lit-glow-r2"],
    ["#lit-cn", "#lit-glow-cn"],
    ["#lit-tail", "#lit-glow-tail"],
  ];
  for (const pair of litPairs) {
    for (const sel of pair) {
      const el = document.querySelector<SVGPathElement>(sel)!;
      const len = el.getTotalLength();
      el.style.strokeDasharray = `${len}`;
      el.style.strokeDashoffset = `${len}`;
    }
  }
  const draw = (sel: string[], from: number, to: number, dur: number, pos: number, tl: gsap.core.Timeline) => {
    for (const s of sel) {
      const el = document.querySelector<SVGPathElement>(s)!;
      const len = el.getTotalLength();
      tl.fromTo(
        el,
        { strokeDashoffset: len * (1 - from) },
        { strokeDashoffset: len * (1 - to), duration: dur, ease: "none" },
        pos
      );
    }
  };

  // card k fades in at these timeline positions (of 100)
  const STAGE_BOUNDS = [0.116, 0.276, 0.456, 0.636, 0.796];
  const stageFromProgress = (p: number) =>
    STAGE_BOUNDS.reduce((k, bound) => (p >= bound ? k + 1 : k), 0);

  const tl = gsap.timeline({
    defaults: { ease: "none" },
    scrollTrigger: {
      trigger: "#pipeline-scroll",
      start: "top top",
      end: "+=5200",
      pin: "#pipeline-stage",
      scrub: 0.7,
      anticipatePin: 1,
      onUpdate(self) {
        const p = self.progress;
        const k = stageFromProgress(p);
        index.textContent = `0${k + 1}`;
        bars.forEach((b, i) => b.classList.toggle("is-on", i <= k));
        const lit = (i: number, on: boolean) => {
          document.querySelector(nodes[i])?.classList.toggle("is-lit", on);
          labels[i]?.classList.toggle("is-lit", on);
        };
        lit(0, p > 0.02);
        lit(1, p > 0.2);
        lit(2, p > 0.4);
        document.querySelector("#node-cn")?.classList.toggle("is-lit", p > 0.56);
        lit(3, p > 0.56);
        lit(4, p > 0.76);
        lit(5, p > 0.92);
      },
    },
  });

  const card = (k: number, pos: number) => {
    if (k > 0) tl.to(cards[k - 1], { autoAlpha: 0, y: 18, duration: 2.4 }, pos);
    tl.fromTo(
      cards[k],
      { autoAlpha: 0, y: 24 },
      { autoAlpha: 1, y: 0, duration: 2.6 },
      pos + 1.6
    );
  };

  // -- timeline body (100 duration units; zero tween pins the total) --
  tl.to("#pipeline-stage", { duration: 0 }, 100);

  // s0: the packet wakes up at the upstream node
  tl.fromTo("#packet", { autoAlpha: 0, scale: 0.4 }, { autoAlpha: 1, scale: 1, duration: 3 }, 1);
  tl.to("#packet", {
    motionPath: { path: "#lit-1", align: "#lit-1", alignOrigin: [0.5, 0.5], start: 0, end: 0.001 },
    duration: 0.01,
  }, 0);

  // s1: draw to the probe; pulse rings sweep
  card(1, 10);
  draw(["#lit-1", "#lit-glow-1"], 0, 0.5, 9, 10, tl);
  tl.to("#packet", {
    motionPath: { path: "#lit-1", align: "#lit-1", alignOrigin: [0.5, 0.5], start: 0.001, end: 0.5 },
    duration: 9,
  }, 10);
  tl.fromTo("#pulse-a", { attr: { r: 36 }, opacity: 0.8 }, { attr: { r: 96 }, opacity: 0, duration: 6, immediateRender: false }, 15);
  tl.fromTo("#pulse-b", { attr: { r: 36 }, opacity: 0.8 }, { attr: { r: 96 }, opacity: 0, duration: 6, immediateRender: false }, 18.5);

  // s2: continue to the release node; shards join the packet
  card(2, 26);
  draw(["#lit-1", "#lit-glow-1"], 0.5, 1, 10, 27, tl);
  tl.to("#packet", {
    motionPath: { path: "#lit-1", align: "#lit-1", alignOrigin: [0.5, 0.5], start: 0.5, end: 1 },
    duration: 10,
  }, 27);
  tl.fromTo("#packet img:first-child", { scale: 1 }, { scale: 1.3, yoyo: true, repeat: 1, duration: 1.6 }, 36);

  // s3: split onto the two mirror branches
  card(3, 44);
  draw(["#lit-r2", "#lit-glow-r2"], 0, 0.55, 9, 45, tl);
  draw(["#lit-cn", "#lit-glow-cn"], 0, 0.55, 9, 45, tl);
  tl.to("#packet", {
    motionPath: { path: "#lit-r2", align: "#lit-r2", alignOrigin: [0.5, 0.5], start: 0, end: 0.55 },
    duration: 9,
  }, 45);
  tl.fromTo("#packet-2", { autoAlpha: 0 }, { autoAlpha: 1, duration: 1.5 }, 45);
  tl.to("#packet-2", {
    motionPath: { path: "#lit-cn", align: "#lit-cn", alignOrigin: [0.5, 0.5], start: 0, end: 0.55 },
    duration: 9,
  }, 45);

  // s4: converge on the router
  card(4, 62);
  draw(["#lit-r2", "#lit-glow-r2"], 0.55, 1, 8, 63, tl);
  draw(["#lit-cn", "#lit-glow-cn"], 0.55, 1, 8, 63, tl);
  tl.to("#packet", {
    motionPath: { path: "#lit-r2", align: "#lit-r2", alignOrigin: [0.5, 0.5], start: 0.55, end: 1 },
    duration: 8,
  }, 63);
  tl.to("#packet-2", {
    motionPath: { path: "#lit-cn", align: "#lit-cn", alignOrigin: [0.5, 0.5], start: 0.55, end: 1 },
    duration: 8,
  }, 63);
  tl.to("#packet-2", { autoAlpha: 0, scale: 0.5, duration: 2 }, 71.5);

  // s5: the tail — delta lands on the desktop
  card(5, 78);
  draw(["#lit-tail", "#lit-glow-tail"], 0, 1, 7, 80, tl);
  tl.to("#packet", {
    motionPath: { path: "#lit-tail", align: "#lit-tail", alignOrigin: [0.5, 0.5], start: 0, end: 1 },
    duration: 7,
  }, 80);
  tl.to("#packet", { scale: 0.3, autoAlpha: 0, duration: 2.5 }, 88);

  // finale
  tl.fromTo("#pipe-finale", { autoAlpha: 0 }, { autoAlpha: 1, duration: 5 }, 93);
  tl.fromTo(
    "#pipe-finale .chip",
    { scale: 0.6, autoAlpha: 0 },
    { scale: 1, autoAlpha: 1, duration: 3, ease: "back.out(2)" },
    94
  );
  tl.to("#stage-index", { autoAlpha: 0, duration: 3 }, 93);

  return tl;
}

/* ---- keep ScrollTrigger honest once webfonts settle ---- */
document.fonts?.ready.then(() => ScrollTrigger.refresh());
