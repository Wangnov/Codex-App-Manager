import { afterEach, describe, expect, it } from "vitest";

import {
  installContextMenuPolicy,
  isEditableContextTarget,
} from "./contextMenuPolicy";

describe("isEditableContextTarget", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("accepts text inputs and textareas", () => {
    document.body.innerHTML = `
      <input id="t" type="text" />
      <textarea id="a"></textarea>
      <div id="ce" contenteditable="true"></div>
    `;
    expect(isEditableContextTarget(document.getElementById("t"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("a"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("ce"))).toBe(true);
  });

  it("rejects non-text inputs and ordinary chrome", () => {
    document.body.innerHTML = `
      <input id="cb" type="checkbox" />
      <button id="b">go</button>
      <div id="d">plain</div>
    `;
    expect(isEditableContextTarget(document.getElementById("cb"))).toBe(false);
    expect(isEditableContextTarget(document.getElementById("b"))).toBe(false);
    expect(isEditableContextTarget(document.getElementById("d"))).toBe(false);
    expect(isEditableContextTarget(null)).toBe(false);
  });

  it("walks up to an editable ancestor", () => {
    document.body.innerHTML = `<div contenteditable="true"><span id="s">x</span></div>`;
    expect(isEditableContextTarget(document.getElementById("s"))).toBe(true);
  });
});

describe("installContextMenuPolicy", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("prevents default on non-editable targets when enabled", () => {
    document.body.innerHTML = `<div id="d">x</div><input id="t" type="text" />`;
    const dispose = installContextMenuPolicy(true);

    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(true);

    const edit = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("t")!.dispatchEvent(edit);
    expect(edit.defaultPrevented).toBe(false);

    dispose();
  });

  it("is a no-op when disabled (dev builds)", () => {
    document.body.innerHTML = `<div id="d">x</div>`;
    const dispose = installContextMenuPolicy(false);
    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
    dispose();
  });

  it("dispose removes the listener", () => {
    document.body.innerHTML = `<div id="d">x</div>`;
    const dispose = installContextMenuPolicy(true);
    dispose();
    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
  });
});
