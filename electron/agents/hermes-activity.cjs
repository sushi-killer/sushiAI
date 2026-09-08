// Translate confirmed native effects into user-facing activity. A tool start,
// an agent's prose, or a staged approval is never evidence of a saved change.
function activityFor(event, item) {
  if (
    event.type === "review.summary" &&
    typeof event.payload?.text === "string" &&
    event.payload.text.trim()
  ) {
    return {
      kind: "self-improvement",
      title: "Self-improvement",
      summary: event.payload.text.slice(0, 4000),
    };
  }
  if (event.type !== "tool.complete" || !item || item.status === "error")
    return null;
  const result = item.output;
  if (
    !result ||
    typeof result !== "object" ||
    result.success !== true ||
    result.staged === true ||
    result.changed === false ||
    result.noop === true ||
    result.status === "unchanged"
  )
    return null;
  const input = item.input || {};
  if (item.name === "memory") {
    // Hermes treats duplicate adds as successful, but no memory was written.
    if (result.message === "Entry already exists (no duplicate added).")
      return null;
    const operations = Array.isArray(input.operations)
      ? input.operations
      : [input];
    const writes = operations.filter(
      (op) => op && ["add", "replace", "remove"].includes(op.action),
    );
    if (!writes.length) return null;
    const target =
      input.target === "user" || input.target === "profile"
        ? "User memory"
        : "Memory";
    const actions = [
      ...new Set(
        writes.map(
          (op) =>
            ({ add: "added", replace: "updated", remove: "removed" })[
              op.action
            ],
        ),
      ),
    ];
    return {
      kind: "memory",
      title: `${target} ${actions.join(" / ")}`,
      // Native batches may contain idempotent adds; their operation count is
      // not evidence of the number of entries actually changed.
      summary: Array.isArray(input.operations)
        ? `${writes.length} memory operations completed. Duplicate entries may be unchanged.`
        : "Memory write completed.",
      toolId: item.id,
    };
  }
  if (item.name === "skill_manage") {
    const verbs = {
      create: "Skill created",
      patch: "Skill improved",
      edit: "Skill updated",
      delete: "Skill removed",
      write_file: "Skill file written",
      remove_file: "Skill file removed",
    };
    if (!verbs[input.action]) return null;
    return {
      kind: "skills",
      title: verbs[input.action],
      summary: String(
        input.name || input.skill_name || result.name || "Skill change saved.",
      ).slice(0, 1000),
      toolId: item.id,
    };
  }
  return null;
}
module.exports = { activityFor };
