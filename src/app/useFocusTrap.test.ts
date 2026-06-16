import { describe, expect, it } from "vitest";

import { cycleFocusTarget } from "./useFocusTrap";

describe("cycleFocusTarget", () => {
  it("returns null for empty lists and middle items", () => {
    const first = document.createElement("button");
    const second = document.createElement("button");
    const third = document.createElement("button");

    expect(cycleFocusTarget([], null, false)).toBeNull();
    expect(cycleFocusTarget([first, second, third], second, false)).toBeNull();
    expect(cycleFocusTarget([first, second, third], second, true)).toBeNull();
  });

  it("wraps backward from the first item and forward from the last item", () => {
    const first = document.createElement("button");
    const last = document.createElement("button");

    expect(cycleFocusTarget([first, last], first, true)).toBe(last);
    expect(cycleFocusTarget([first, last], last, false)).toBe(first);
  });
});
