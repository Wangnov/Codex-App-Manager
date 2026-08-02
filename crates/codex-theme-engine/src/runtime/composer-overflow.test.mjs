import assert from "node:assert/strict";

import { test } from "vitest";

import {
  createComposerOverflowAnnotator,
  selectComposerSurfaces,
} from "./composer-overflow.mjs";

const OVERFLOW_ATTR = "data-cts-composer-overflow";
const MODE_ATTR = "data-cts-composer-mode";

class FakeNode {
  constructor(tagName, { className = "", attributes = {}, nativeStyle = {} } = {}) {
    this.tagName = tagName.toUpperCase();
    this.className = className;
    this.attributes = new Map(Object.entries(attributes));
    this.nativeStyle = { overflowY: "visible", maxHeight: "none", ...nativeStyle };
    this.children = [];
    this.parentElement = null;
  }

  append(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  contains(candidate) {
    for (let node = candidate; node; node = node.parentElement) {
      if (node === this) return true;
    }
    return false;
  }

  querySelector(selector) {
    const descendants = [];
    const visit = (node) => {
      for (const child of node.children) {
        descendants.push(child);
        visit(child);
      }
    };
    visit(this);
    if (selector.includes("[data-codex-composer]")) {
      const marked = descendants.find((node) => node.getAttribute("data-codex-composer") !== null);
      if (marked) return marked;
    }
    return descendants.find((node) =>
      node.className.split(/\s+/).includes("ProseMirror") ||
      node.getAttribute("contenteditable") === "true" ||
      node.tagName === "TEXTAREA") ?? null;
  }
}

test("surface selection prefers the marked primary Composer over static cards", () => {
  const staticCard = { querySelector: () => null };
  const primaryComposer = { querySelector: () => ({}) };
  const marker = { closest: () => primaryComposer };
  const root = {
    querySelectorAll(selector) {
      if (selector === "[data-codex-composer]") return [marker];
      if (selector === "[data-codex-composer-root] .composer-surface-chrome") return [];
      if (selector === ".composer-surface-chrome") return [staticCard, primaryComposer];
      return [];
    },
  };

  assert.deepEqual(selectComposerSurfaces(root), [primaryComposer]);
});

test("surface selection fallback excludes surfaces without an editor", () => {
  const staticSurface = { querySelector: () => null };
  const editableSurface = { querySelector: () => ({}) };
  const root = {
    querySelectorAll(selector) {
      if (selector === ".composer-surface-chrome") return [staticSurface, editableSurface];
      return [];
    },
  };

  assert.deepEqual(selectComposerSurfaces(root), [editableSurface]);
});

test("multiline roles are cleared when React reuses the nodes for single-line layout", () => {
  const shell = new FakeNode("div", {
    className: "composer-surface-chrome multiline",
    nativeStyle: { overflowY: "auto" },
  });
  const lane = shell.append(new FakeNode("div", {
    className: "lane multiline",
    nativeStyle: { overflowY: "auto" },
  }));
  const editor = lane.append(new FakeNode("div", {
    className: "editor multiline",
    nativeStyle: { overflowY: "auto", maxHeight: "160px" },
  }));
  editor.append(new FakeNode("div", {
    className: "ProseMirror",
    attributes: { contenteditable: "true", "data-codex-composer": "true" },
  }));

  const readStyle = (node) => {
    const role = node.getAttribute(OVERFLOW_ATTR);
    if (role === "shell") return { ...node.nativeStyle, overflowY: "clip" };
    if (role === "lane") return { ...node.nativeStyle, overflowY: "visible" };
    if (role === "editor") return { ...node.nativeStyle, overflowY: "auto" };
    return node.nativeStyle;
  };
  const annotate = createComposerOverflowAnnotator({
    overflowAttribute: OVERFLOW_ATTR,
    modeAttribute: MODE_ATTR,
    readStyle,
    viewportSignature: () => "1280x800",
  });

  annotate([shell]);
  assert.equal(shell.getAttribute(OVERFLOW_ATTR), "shell");
  assert.equal(shell.getAttribute(MODE_ATTR), "scrolling");
  assert.equal(lane.getAttribute(OVERFLOW_ATTR), "lane");
  assert.equal(editor.getAttribute(OVERFLOW_ATTR), "editor");

  shell.className = "composer-surface-chrome single-line";
  lane.className = "lane single-line";
  editor.className = "editor single-line";
  shell.nativeStyle = { overflowY: "visible", maxHeight: "none" };
  lane.nativeStyle = { overflowY: "visible", maxHeight: "none" };
  editor.nativeStyle = { overflowY: "hidden", maxHeight: "none" };

  annotate([shell]);
  assert.equal(shell.getAttribute(OVERFLOW_ATTR), "shell");
  assert.equal(shell.getAttribute(MODE_ATTR), "single-line");
  assert.equal(lane.getAttribute(OVERFLOW_ATTR), null);
  assert.equal(editor.getAttribute(OVERFLOW_ATTR), null);
  assert.equal(readStyle(editor).overflowY, "hidden");
});

test("26.727 multiline lanes remain detectable when a skin masks native overflow", () => {
  for (const overflowY of ["visible", "hidden", "clip"]) {
    const shell = new FakeNode("div", { className: "composer-surface-chrome" });
    const wrapper = shell.append(new FakeNode("div", {
      className: "grid overflow-hidden",
      nativeStyle: { overflowY: "hidden" },
    }));
    const lane = wrapper.append(new FakeNode("div", {
      className: "mb-1 flex-grow overflow-y-auto",
      nativeStyle: { overflowY },
    }));
    const editor = lane.append(new FakeNode("div", {
      className: "max-h-[25dvh] overflow-y-auto",
      nativeStyle: { overflowY: "auto", maxHeight: "160px" },
    }));
    editor.append(new FakeNode("div", {
      className: "ProseMirror",
      attributes: { contenteditable: "true", "data-codex-composer": "true" },
    }));

    const readStyle = (node) => {
      const role = node.getAttribute(OVERFLOW_ATTR);
      if (role === "shell") return { ...node.nativeStyle, overflowY: "clip" };
      if (role === "lane") return { ...node.nativeStyle, overflowY: "visible" };
      if (role === "editor") return { ...node.nativeStyle, overflowY: "auto" };
      return node.nativeStyle;
    };
    const annotate = createComposerOverflowAnnotator({
      overflowAttribute: OVERFLOW_ATTR,
      modeAttribute: MODE_ATTR,
      readStyle,
      viewportSignature: () => "1280x800",
    });

    annotate([shell]);
    assert.equal(shell.getAttribute(MODE_ATTR), "scrolling");
    assert.equal(lane.getAttribute(OVERFLOW_ATTR), "lane", overflowY);
    assert.equal(readStyle(lane).overflowY, "visible", overflowY);
    assert.equal(editor.getAttribute(OVERFLOW_ATTR), "editor", overflowY);
    assert.equal(wrapper.getAttribute(OVERFLOW_ATTR), null, overflowY);
  }
});

test("unchanged layout reuses its classification without remeasuring hardened styles", () => {
  const shell = new FakeNode("div", { className: "composer-surface-chrome" });
  const editor = shell.append(new FakeNode("div", {
    className: "editor",
    nativeStyle: { overflowY: "auto", maxHeight: "120px" },
  }));
  editor.append(new FakeNode("div", {
    className: "ProseMirror",
    attributes: { "data-codex-composer": "true" },
  }));
  let reads = 0;
  const annotate = createComposerOverflowAnnotator({
    overflowAttribute: OVERFLOW_ATTR,
    modeAttribute: MODE_ATTR,
    readStyle: (node) => {
      reads += 1;
      return node.nativeStyle;
    },
    viewportSignature: () => "1280x800",
  });

  annotate([shell]);
  const initialReads = reads;
  annotate([shell]);
  assert.equal(reads, initialReads);
});

test("external style invalidation remeasures an unchanged Composer path", () => {
  const shell = new FakeNode("div", { className: "composer-surface-chrome" });
  const editor = shell.append(new FakeNode("div", {
    className: "editor",
    nativeStyle: { overflowY: "visible", maxHeight: "none" },
  }));
  editor.append(new FakeNode("div", {
    className: "ProseMirror",
    attributes: { "data-codex-composer": "true" },
  }));
  let reads = 0;
  const annotate = createComposerOverflowAnnotator({
    overflowAttribute: OVERFLOW_ATTR,
    modeAttribute: MODE_ATTR,
    readStyle: (node) => {
      reads += 1;
      return node.nativeStyle;
    },
    viewportSignature: () => "1280x800",
  });

  annotate([shell]);
  assert.equal(shell.getAttribute(MODE_ATTR), "single-line");
  const initialReads = reads;

  // Simulate a stylesheet or ancestor change: computed style changes while
  // the Composer path, viewport, classes, and inline styles remain identical.
  editor.nativeStyle = { overflowY: "auto", maxHeight: "160px" };
  annotate([shell]);
  assert.equal(reads, initialReads);
  assert.equal(shell.getAttribute(MODE_ATTR), "single-line");

  annotate.invalidate();
  annotate([shell]);
  assert.ok(reads > initialReads);
  assert.equal(shell.getAttribute(MODE_ATTR), "scrolling");
  assert.equal(editor.getAttribute(OVERFLOW_ATTR), "editor");
});
