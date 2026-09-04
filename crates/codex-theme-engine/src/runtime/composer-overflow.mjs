// Shared Composer capability detection. These functions are serialized into
// the renderer payload, so keep them self-contained (no module-scope reads).

export function selectComposerSurfaces(root) {
  // Codex 26.730.61309 introduced the CSS-module surface; the adjacent
  // 26.727.51351 still used the legacy class (see docs/composer-compatibility.md).
  // Detect capabilities rather than trusting a version hint or module hash.
  const current = "[data-composer-surface-variant][data-composer-layout]";
  const surfaceSelector = `${current}, .composer-surface-chrome`;
  const editorSelector = '[data-codex-composer], .ProseMirror[contenteditable="true"], ' +
    '[contenteditable="true"], textarea';
  const unique = (nodes) => [...new Set(nodes.filter(Boolean))];
  const ownsEditor = (surface) => {
    if (surface.hasAttribute?.("data-cts-composer-surface-compat") && !surface.matches(current)) {
      return false;
    }
    const editor = surface.querySelector(editorSelector);
    // Nested surfaces and static utility/review cards must not get a second
    // frame. Prefer the surface immediately enclosing the actual editor.
    return Boolean(editor && (!editor.closest || editor.closest(surfaceSelector) === surface));
  };
  const marked = unique(
    [...root.querySelectorAll("[data-codex-composer]")]
      .map((node) => node.closest?.(surfaceSelector)),
  );
  const rooted = unique([
    ...root.querySelectorAll(`[data-codex-composer-root] ${current}`),
    ...root.querySelectorAll("[data-codex-composer-root] .composer-surface-chrome"),
  ]);
  const preferred = unique([...marked, ...rooted]).filter(ownsEditor);
  if (preferred.length) return preferred;

  // Older renderers may not expose the stable Composer markers. Retain a
  // capability fallback, but exclude static surfaces that contain no editor.
  return unique([
    ...root.querySelectorAll(current),
    ...root.querySelectorAll(".composer-surface-chrome"),
  ]).filter(ownsEditor);
}

export function clearComposerSurfaceCompat(root) {
  for (const node of root.querySelectorAll("[data-cts-composer-surface-compat]")) {
    node.classList.remove("composer-surface-chrome");
    node.removeAttribute("data-cts-composer-surface-compat");
  }
}

export function reconcileComposerSurfaces(root) {
  const surfaces = selectComposerSurfaces(root);
  const desired = new Set(surfaces.filter((node) =>
    node.matches("[data-composer-surface-variant][data-composer-layout]")));
  // Own only aliases we add, so cleanup never removes a native legacy class.
  // Reconcile on route changes and when React reuses or replaces the surface.
  for (const node of root.querySelectorAll("[data-cts-composer-surface-compat]")) {
    if (!desired.has(node)) {
      node.classList.remove("composer-surface-chrome");
      node.removeAttribute("data-cts-composer-surface-compat");
    }
  }
  for (const node of desired) {
    if (!node.classList.contains("composer-surface-chrome")) {
      node.classList.add("composer-surface-chrome");
      node.setAttribute("data-cts-composer-surface-compat", "true");
    }
  }
  return surfaces;
}

export function createComposerOverflowAnnotator({
  overflowAttribute,
  modeAttribute,
  readStyle,
  viewportSignature,
}) {
  let cache = new WeakMap();
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
    const layout = node.getAttribute("data-composer-layout") || "";
    const overflow = node.getAttribute("data-composer-surface-overflow") || "";
    return `${node.tagName || ""}\u0000${className}\u0000${node.getAttribute("style") || ""}` +
      `\u0000${layout}\u0000${overflow}`;
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

  const annotate = (composers) => {
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

  // Computed overflow can change without altering any node in the Composer
  // path (for example through an ancestor class, stylesheet, or media query).
  // Let the runtime discard structural classifications when those external
  // style inputs change while retaining the cache for ordinary ensure passes.
  annotate.invalidate = () => {
    cache = new WeakMap();
  };

  return annotate;
}
