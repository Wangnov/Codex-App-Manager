// Shared Composer capability detection. These functions are serialized into
// the renderer payload, so keep them self-contained (no module-scope reads).

export function selectComposerSurfaces(root) {
  const unique = (nodes) => [...new Set(nodes.filter(Boolean))];
  const marked = unique(
    [...root.querySelectorAll("[data-codex-composer]")]
      .map((node) => node.closest?.(".composer-surface-chrome")),
  );
  const rooted = unique([
    ...root.querySelectorAll("[data-codex-composer-root] .composer-surface-chrome"),
  ]);
  const preferred = unique([...marked, ...rooted]);
  if (preferred.length) return preferred;

  // Older renderers may not expose the stable Composer markers. Retain a
  // capability fallback, but exclude static surfaces that contain no editor.
  return [...root.querySelectorAll(".composer-surface-chrome")].filter((surface) =>
    surface.querySelector(
      '.ProseMirror[contenteditable="true"], [contenteditable="true"], textarea',
    ));
}

export function createComposerOverflowAnnotator({
  overflowAttribute,
  modeAttribute,
  readStyle,
  viewportSignature,
}) {
  const cache = new WeakMap();
  const annotatedNodes = new Set();
  const modeNodes = new Set();

  const setAttribute = (node, name, value) => {
    if (node.getAttribute(name) !== value) node.setAttribute(name, value);
  };

  const pathMatches = (left, right) =>
    left.length === right.length && left.every((node, index) => node === right[index]);

  const declaresScrollableOverflow = (node) => {
    const className = typeof node.className === "string" ? node.className : "";
    return className.split(/\s+/).some((token) =>
      /^!?overflow-y-(?:auto|scroll)$/.test(token));
  };

  const nodeSignature = (node) => {
    const className = typeof node.className === "string" ? node.className : "";
    return `${node.tagName || ""}\u0000${className}\u0000${node.getAttribute("style") || ""}`;
  };

  const classify = (composer, path, signature) => {
    // Runtime roles change computed overflow through the hardening stylesheet.
    // Clear them only when the structural signature changes, measure native
    // capabilities, then restore before applying the guarded final diff.
    const previousRoles = new Map();
    for (const node of annotatedNodes) {
      if (node === composer || composer.contains(node)) {
        const role = node.getAttribute(overflowAttribute);
        if (role !== null) previousRoles.set(node, role);
        node.removeAttribute(overflowAttribute);
      }
    }
    const previousMode = composer.getAttribute(modeAttribute);
    if (previousMode !== null) composer.removeAttribute(modeAttribute);

    let fallback = null;
    let editorScrollRoot = null;
    const nativeOverflow = new Map();
    for (const node of path) {
      const style = readStyle(node);
      const scrollable = /^(auto|scroll)$/.test(style.overflowY);
      const maxHeight = Number.parseFloat(style.maxHeight);
      const finiteHeight = style.maxHeight !== "none" &&
        Number.isFinite(maxHeight) && maxHeight > 0;
      nativeOverflow.set(node, style.overflowY);
      if (scrollable && !fallback) fallback = node;
      if (scrollable && finiteHeight) {
        editorScrollRoot = node;
        break;
      }
    }
    editorScrollRoot ??= fallback;

    const roles = new Map([[composer, "shell"]]);
    if (editorScrollRoot) {
      roles.set(editorScrollRoot, "editor");
      for (let node = editorScrollRoot.parentElement;
        node && node !== composer;
        node = node.parentElement) {
        const overflowY = nativeOverflow.has(node)
          ? nativeOverflow.get(node)
          : readStyle(node).overflowY;
        // A skin can mask the app's lane overflow before we measure it. Keep
        // the app's explicit utility class as a structural signal, without
        // treating unrelated intermediate layout wrappers as lanes.
        if (/^(auto|scroll)$/.test(overflowY) || declaresScrollableOverflow(node)) {
          roles.set(node, "lane");
        }
      }
    }

    for (const [node, role] of previousRoles) setAttribute(node, overflowAttribute, role);
    if (previousMode !== null) setAttribute(composer, modeAttribute, previousMode);

    const value = {
      path,
      signature,
      roles,
      mode: editorScrollRoot ? "scrolling" : "single-line",
    };
    cache.set(composer, value);
    return value;
  };

  return (composers) => {
    const desiredRoles = new Map();
    const desiredModes = new Map();

    for (const composer of composers) {
      const editable = composer.querySelector(
        '[data-codex-composer], .ProseMirror[contenteditable="true"], ' +
        '[contenteditable="true"], textarea',
      );
      if (!editable) continue;

      const path = [];
      for (let node = editable; node && node !== composer; node = node.parentElement) {
        path.push(node);
      }
      if (!path.length || path.at(-1)?.parentElement !== composer) continue;

      const signature = `${viewportSignature()}\u0001${[composer, ...path]
        .map(nodeSignature).join("\u0002")}`;
      const previous = cache.get(composer);
      const classification = previous && previous.signature === signature &&
        pathMatches(previous.path, path)
        ? previous
        : classify(composer, path, signature);

      for (const [node, role] of classification.roles) desiredRoles.set(node, role);
      desiredModes.set(composer, classification.mode);
    }

    for (const node of annotatedNodes) {
      if (!desiredRoles.has(node)) {
        node.removeAttribute(overflowAttribute);
        annotatedNodes.delete(node);
      }
    }
    for (const [node, role] of desiredRoles) {
      setAttribute(node, overflowAttribute, role);
      annotatedNodes.add(node);
    }

    for (const node of modeNodes) {
      if (!desiredModes.has(node)) {
        node.removeAttribute(modeAttribute);
        modeNodes.delete(node);
      }
    }
    for (const [node, mode] of desiredModes) {
      setAttribute(node, modeAttribute, mode);
      modeNodes.add(node);
    }
  };
}
