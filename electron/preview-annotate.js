// Injected by electron/preview.cjs into an HTML artifact shown in the
// Preview pane. The page runs in a sandboxed frame with an opaque origin, so
// this script can only tell the parent what the owner pointed at; it never
// receives anything but a pick-mode switch.
(() => {
  const post = (message) =>
    parent.postMessage({ sushiai: "annotate", ...message }, "*");
  const rectOf = (r) => ({ x: r.left, y: r.top, w: r.width, h: r.height });
  const where = (node) => {
    if (/^#\/\d/.test(location.hash)) return "slide " + location.hash;
    let el = node && node.nodeType === 1 ? node : node && node.parentElement;
    for (; el; el = el.previousElementSibling || el.parentElement) {
      const heading = el.matches?.("h1,h2,h3,h4,h5,h6")
        ? el
        : el.querySelector?.("h1,h2,h3,h4,h5,h6");
      if (heading && heading.textContent.trim())
        return heading.textContent.trim().slice(0, 120);
    }
    return "";
  };
  // An SVG shape or a bare wrapper says little; the nearest element with an id
  // or a class (a Mermaid node, a slide block) is what the owner means.
  const PRIMITIVE =
    /^(path|rect|circle|ellipse|line|polyline|polygon|text|tspan|foreignobject|span|p|b|i|em|strong|code)$/i;
  const container = (el) => {
    for (
      let node = el, depth = 0;
      node && node !== document.body && depth < 8;
      node = node.parentElement, depth++
    )
      if ((node.id || node.classList.length) && !PRIMITIVE.test(node.tagName))
        return node;
    return el;
  };
  const part = (node) => {
    if (node.id) return `${node.tagName.toLowerCase()}#${CSS.escape(node.id)}`;
    let name = node.tagName.toLowerCase();
    const classes = [...node.classList]
      .slice(0, 2)
      .map((c) => "." + CSS.escape(c))
      .join("");
    const parentNode = node.parentElement;
    if (
      parentNode &&
      [...parentNode.children].filter((c) => c.tagName === node.tagName)
        .length > 1
    )
      name += `:nth-of-type(${[...parentNode.children].filter((c) => c.tagName === node.tagName).indexOf(node) + 1})`;
    return name + classes;
  };
  // A short CSS path from the nearest id (or body) down to the element.
  const selectorOf = (el) => {
    const parts = [];
    for (
      let node = el;
      node && node !== document.body && parts.length < 5;
      node = node.parentElement
    ) {
      parts.unshift(part(node));
      if (node.id) break;
    }
    return parts.join(" > ");
  };
  let pick = false;
  let hovered = null;
  // The element a comment is being written for keeps its mark until the
  // parent closes the comment box.
  let picked = null;
  const restore = (el) => {
    if (el) el.style.outline = el.dataset.sushiaiOutline || "";
  };
  const unmark = () => {
    if (hovered !== picked) restore(hovered);
    hovered = null;
  };
  const unpick = () => {
    restore(picked);
    picked = null;
  };
  addEventListener("message", (event) => {
    if (event.source !== parent) return;
    if (event.data?.sushiai === "clear") return unpick();
    if (event.data?.sushiai !== "mode") return;
    pick = event.data.pick === true;
    if (!pick) unmark();
  });
  document.addEventListener(
    "mouseover",
    (event) => {
      const target = container(event.target);
      if (!pick || target === picked || target === hovered) return;
      unmark();
      hovered = target;
      hovered.dataset.sushiaiOutline = hovered.style.outline;
      hovered.style.outline = "2px dashed #d6b57a";
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      if (!pick) return;
      event.preventDefault();
      event.stopPropagation();
      const el = container(event.target);
      const text = (el.innerText || el.getAttribute("aria-label") || el.tagName)
        .trim()
        .slice(0, 300);
      unmark();
      unpick();
      picked = el;
      if (!picked.dataset.sushiaiOutline)
        picked.dataset.sushiaiOutline = picked.style.outline;
      picked.style.outline = "2px solid #d6b57a";
      post({
        kind: "element",
        quote: text,
        where: where(el),
        selector: selectorOf(el).slice(0, 300),
        rect: rectOf(el.getBoundingClientRect()),
      });
    },
    true,
  );
  document.addEventListener("mouseup", () => {
    if (pick) return;
    const selection = getSelection();
    const quote = selection && String(selection).trim();
    if (!quote || !selection.rangeCount) return;
    post({
      kind: "text",
      quote: quote.slice(0, 500),
      where: where(selection.anchorNode),
      rect: rectOf(selection.getRangeAt(0).getBoundingClientRect()),
    });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") post({ kind: "cancel" });
  });
})();
