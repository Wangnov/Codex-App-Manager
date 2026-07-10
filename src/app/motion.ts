// Central GSAP setup + the orchestrated home-screen motion.
//
// Importing this module registers every plugin once and defines the project's
// signature easing curves. The platform home views call useHomeMotion() to play
// a choreographed entrance/transition each time the visible "scene" changes.

import { useGSAP } from "@gsap/react";
import gsap from "gsap";
import { CustomEase } from "gsap/CustomEase";
import { DrawSVGPlugin } from "gsap/DrawSVGPlugin";
import { SplitText } from "gsap/SplitText";
import type { RefObject } from "react";

gsap.registerPlugin(useGSAP, CustomEase, DrawSVGPlugin, SplitText);

// Signature curves — smooth deceleration (no tacky bounce); "pop" is a
// restrained overshoot reserved for the focal medallion.
CustomEase.create("cam-out", "0.16, 1, 0.3, 1"); // expo-out settle
CustomEase.create("cam-in-out", "0.65, 0, 0.35, 1");
CustomEase.create("cam-pop", "0.34, 1.3, 0.5, 1"); // gentle overshoot

export { gsap, useGSAP, SplitText };

interface HomeMotionOpts {
  /** Reveal the headline character-by-character (skip for shimmer/loading text,
   *  whose background-clip:text gradient can't survive per-char wrapping). */
  splitHeadline: boolean;
  /** Draw the success checkmark stroke in. */
  success: boolean;
}

/**
 * Choreograph the home hero + details + actions whenever `scene` changes.
 * Scoped to `scope`; gated behind prefers-reduced-motion via matchMedia so
 * reduced-motion users get the static, fully-visible layout.
 */
export function useHomeMotion(
  scope: RefObject<HTMLElement | null>,
  scene: string,
  opts: HomeMotionOpts,
): void {
  const { splitHeadline, success } = opts;
  useGSAP(
    () => {
      const mm = gsap.matchMedia();
      mm.add("(prefers-reduced-motion: no-preference)", () => {
        const q = gsap.utils.selector(scope);
        // Skip aria-hidden placeholders (e.g. the rechecking-state .microcue
        // height spacer) so autoAlpha never reveals them.
        const at = (sel: string) => {
          const els = q(sel).filter((el) => el.getAttribute("aria-hidden") !== "true");
          return els.length ? els : null;
        };
        // Strip GSAP's inline opacity/visibility/transform when each tween ends,
        // so CSS regains control (otherwise inline opacity:1 outranks the
        // .btn:disabled fade and the :active press transform).
        const CLEAR = "transform,opacity,visibility";
        let split: SplitText | undefined;
        const tl = gsap.timeline({
          defaults: { ease: "cam-out", duration: 0.58 },
          // SplitText leaves every glyph in its own inline-block. That is useful
          // during the reveal, but keeping it afterward changes kerning and CJK
          // fallback spacing on Windows. Restore the original text once the
          // complete scene entrance has settled.
          onComplete: () => split?.revert(),
        });
        // fromTo with EXPLICIT end states (not from(): persistent sibling nodes
        // like .actions/.list.meta aren't remounted with the keyed hero, and a
        // bare from() would read a leftover inline opacity:0 as the end target
        // and animate 0→0, leaving them stuck hidden).

        const ring = at(".hero .ring");
        if (ring) {
          tl.fromTo(
            ring,
            { scale: 0.62, autoAlpha: 0 },
            { scale: 1, autoAlpha: 1, ease: "cam-pop", duration: 0.72, clearProps: CLEAR },
            0,
          );
        }

        const headline = q(".hero .headline")[0] as HTMLElement | undefined;
        if (headline && splitHeadline) {
          split = SplitText.create(headline, { type: "chars", aria: "auto" });
          tl.fromTo(
            split.chars,
            { yPercent: 90, autoAlpha: 0 },
            { yPercent: 0, autoAlpha: 1, stagger: 0.034, duration: 0.55, clearProps: CLEAR },
            0.14,
          );
        } else if (headline) {
          tl.fromTo(
            headline,
            { y: 12, autoAlpha: 0 },
            { y: 0, autoAlpha: 1, duration: 0.5, clearProps: CLEAR },
            0.14,
          );
        }

        const reveal = (sel: string, y: number, stagger: number, at0: number) => {
          const els = at(sel);
          if (els) {
            tl.fromTo(
              els,
              { y, autoAlpha: 0 },
              { y: 0, autoAlpha: 1, stagger, duration: 0.5, clearProps: CLEAR },
              at0,
            );
          }
        };
        reveal(".hero .sub, .hero .flow, .hero .microcue, .hero .prov, .hero .desc", 10, 0.06, 0.24);
        reveal(".hero .pctbig, .hero .bar, .hero .dlmeta", 10, 0.07, 0.26);
        reveal(".list.meta .row", 12, 0.07, 0.32);
        reveal(".actions > *", 14, 0.08, 0.38);

        if (success) {
          const check = at(".hero .ring svg polyline");
          if (check) {
            tl.fromTo(check, { drawSVG: "0% 0%" }, { drawSVG: "0% 100%", duration: 0.5 }, 0.46);
          }
        }

        return () => {
          split?.revert();
        };
      });
    },
    { scope, dependencies: [scene], revertOnUpdate: true },
  );
}
