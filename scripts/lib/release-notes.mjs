// Pure predicate, split out of check-conventions.mjs so it's testable
// without spawning git or depending on real repo history/depth - see
// tests/check-conventions.test.cjs.
export function addsMarkdownFragment(diffNameStatusOutput) {
  return diffNameStatusOutput.split("\n").some((line) => line.endsWith(".md"));
}
