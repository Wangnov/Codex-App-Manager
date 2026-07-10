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
      <input id="n" type="number" />
      <textarea id="a"></textarea>
      <div id="ce" contenteditable="true">hello</div>
      <div id="tb" role="textbox">x</div>
    `;
    expect(isEditableContextTarget(document.getElementById("t"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("n"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("a"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("ce"))).toBe(true);
    expect(isEditableContextTarget(document.getElementById("tb"))).toBe(true);
  });

  it("accepts Text nodes inside contenteditable", () => {
    document.body.innerHTML = `<div id="ce" contenteditable="true">hello</div>`;
    const text = document.getElementById("ce")!.firstChild;
    expect(text).toBeInstanceOf(Text);
    expect(isEditableContextTarget(text)).toBe(true);
  });

  it("rejects non-text inputs, select, and ordinary chrome", () => {
    document.body.innerHTML = `
      <input id="cb" type="checkbox" />
      <button id="b">go</button>
      <select id="s"><option>a</option></select>
      <div id="d">plain</div>
    `;
    expect(isEditableContextTarget(document.getElementById("cb"))).toBe(false);
    expect(isEditableContextTarget(document.getElementById("b"))).toBe(false);
    expect(isEditableContextTarget(document.getElementById("s"))).toBe(false);
    expect(isEditableContextTarget(document.getElementById("d"))).toBe(false);
    expect(isEditableContextTarget(null)).toBe(false);
  });

  it("walks up to an editable ancestor", () => {
    document.body.innerHTML = `<div contenteditable="true"><span id="s">x</span></div>`;
    expect(isEditableContextTarget(document.getElementById("s"))).toBe(true);
  });
});

describe("installContextMenuPolicy", () => {
  let dispose: (() => void) | null = null;

  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.innerHTML = "";
  });

  it("prevents default on non-editable targets when enabled", () => {
    document.body.innerHTML = `<div id="d">x</div><input id="t" type="text" />`;
    dispose = installContextMenuPolicy(true);

    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(true);

    const edit = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("t")!.dispatchEvent(edit);
    expect(edit.defaultPrevented).toBe(false);
  });

  it("does not prevent default for Text inside contenteditable", () => {
    document.body.innerHTML = `<div id="ce" contenteditable="true">hello</div>`;
    dispose = installContextMenuPolicy(true);
    const text = document.getElementById("ce")!.firstChild!;
    const edit = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    text.dispatchEvent(edit);
    expect(edit.defaultPrevented).toBe(false);
  });

  it("is a no-op when disabled (dev builds)", () => {
    document.body.innerHTML = `<div id="d">x</div>`;
    dispose = installContextMenuPolicy(false);
    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
  });

  it("dispose removes the listener", () => {
    document.body.innerHTML = `<div id="d">x</div>`;
    dispose = installContextMenuPolicy(true);
    dispose();
    dispose = null;
    const plain = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("d")!.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
  });
});
