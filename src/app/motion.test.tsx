import { render } from "@testing-library/react";
import { useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const revert = vi.fn();
  const headline = { getAttribute: vi.fn(() => null) };
  const fromTo = vi.fn();
  const timeline = { fromTo };

  return {
    fromTo,
    headline,
    revert,
    timeline,
    timelineOptions: undefined as { onComplete?: () => void } | undefined,
  };
});

vi.mock("@gsap/react", () => ({
  useGSAP: (setup: () => void) => setup(),
}));

vi.mock("gsap", () => ({
  default: {
    matchMedia: () => ({ add: (_query: string, setup: () => void) => setup() }),
    registerPlugin: vi.fn(),
    timeline: (options: { onComplete?: () => void }) => {
      mocks.timelineOptions = options;
      return mocks.timeline;
    },
    utils: {
      selector: () => (selector: string) =>
        selector === ".hero .headline" ? [mocks.headline] : [],
    },
  },
}));

vi.mock("gsap/CustomEase", () => ({
  CustomEase: { create: vi.fn() },
}));

vi.mock("gsap/DrawSVGPlugin", () => ({
  DrawSVGPlugin: {},
}));

vi.mock("gsap/SplitText", () => ({
  SplitText: {
    create: vi.fn(() => ({ chars: [mocks.headline], revert: mocks.revert })),
  },
}));

import { useHomeMotion } from "./motion";

function Harness() {
  const scope = useRef<HTMLDivElement>(null);
  useHomeMotion(scope, "external", { splitHeadline: true, success: false });
  return <div ref={scope} />;
}

describe("useHomeMotion", () => {
  beforeEach(() => {
    mocks.fromTo.mockClear();
    mocks.revert.mockClear();
    mocks.timelineOptions = undefined;
  });

  it("restores the original headline markup after the scene entrance completes", () => {
    render(<Harness />);

    expect(mocks.revert).not.toHaveBeenCalled();
    expect(mocks.timelineOptions?.onComplete).toBeTypeOf("function");

    mocks.timelineOptions?.onComplete?.();

    expect(mocks.revert).toHaveBeenCalledOnce();
  });
});
