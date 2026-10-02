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
  let pick = false;
  let hovered = null;
  const unmark = () => {
    if (hovered) hovered.style.outline = hovered.dataset.sushiaiOutline || "";
    hovered = null;
  };
  addEventListener("message", (event) => {
    if (event.source !== parent || event.data?.sushiai !== "mode") return;
    pick = event.data.pick === true;
    if (!pick) unmark();
  });
  document.addEventListener(
    "mouseover",
    (event) => {
      if (!pick) return;
      unmark();
      hovered = event.target;
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
      const el = event.target;
      const text = (el.innerText || el.getAttribute("aria-label") || el.tagName)
        .trim()
        .slice(0, 300);
      unmark();
      post({
        kind: "element",
        quote: text,
        where: where(el),
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
