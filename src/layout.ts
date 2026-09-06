import type { Layout } from "./types";
export const uid = () => crypto.randomUUID();
export const leaf = (id: string): Layout => ({ type: "leaf", id });
export const split = (
  a: Layout,
  b: Layout,
  axis: "row" | "column",
  ratio = 0.5,
): Layout => ({ type: "split", id: uid(), axis, ratio, a, b });
export function remove(node: Layout | null, id: string): Layout | null {
  if (!node || node.type === "leaf") return node?.id === id ? null : node;
  const a = remove(node.a, id),
    b = remove(node.b, id);
  return a && b ? { ...node, a, b } : a || b;
}
export function resize(node: Layout, id: string, ratio: number): Layout {
  if (node.type === "leaf") return node;
  return node.id === id
    ? { ...node, ratio }
    : { ...node, a: resize(node.a, id, ratio), b: resize(node.b, id, ratio) };
}
export function insert(
  node: Layout,
  target: string,
  source: string,
  edge: string,
): Layout {
  if (node.type === "leaf") {
    if (node.id !== target) return node;
    const before = edge === "left" || edge === "top";
    return split(
      before ? leaf(source) : node,
      before ? node : leaf(source),
      edge === "left" || edge === "right" ? "row" : "column",
    );
  }
  return {
    ...node,
    a: insert(node.a, target, source, edge),
    b: insert(node.b, target, source, edge),
  };
}
export function swap(node: Layout, a: string, b: string): Layout {
  if (node.type === "leaf")
    return leaf(node.id === a ? b : node.id === b ? a : node.id);
  return { ...node, a: swap(node.a, a, b), b: swap(node.b, a, b) };
}
export function tidy(ids: string[]): Layout | null {
  if (!ids.length) return null;
  if (ids.length === 1) return leaf(ids[0]);
  const [first, ...rest] = ids;
  const stack = (items: string[]): Layout =>
    items.length === 1
      ? leaf(items[0])
      : split(
          leaf(items[0]),
          stack(items.slice(1)),
          "column",
          1 / items.length,
        );
  return split(leaf(first), stack(rest), "row", ids.length > 2 ? 0.53 : 0.5);
}
