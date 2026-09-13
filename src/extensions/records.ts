import type { RecordField, RecordFilter } from "./types.ts";

export type Item = { id: string } & Record<string, unknown>;

/** The values a record needs to satisfy the view it was created in. A view
 * filtered to one value would otherwise hide every record added to it: the
 * compose box writes the primary field and nothing else, so the field the
 * filter tests is missing and the test fails. */
export function seedForFilter(
  filter: RecordFilter[] | undefined,
  primary: string,
): Record<string, unknown> {
  return Object.fromEntries(
    (filter || [])
      .filter((rule) => rule.op === "eq" && rule.field !== primary)
      .map((rule) => [rule.field, rule.value]),
  );
}

export const text = (value: unknown) =>
  typeof value === "string" ? value : "";
export const truthy = (value: unknown) =>
  value !== undefined && value !== null && value !== false && value !== "";

/** A field's value as a person reads it: a select shows its label, a date how
 * far away it is, a flag only when it is set. */
export function display(value: unknown, field?: RecordField): string {
  if (!field) return text(value);
  if (field.type === "select")
    return (
      field.options?.find((option) => option.value === value)?.label ||
      text(value)
    );
  if (field.type === "boolean") return value ? field.label : "";
  if (field.type === "date") return relativeDate(text(value));
  return text(value);
}

/** What the value means, so the app can colour it. A date carries its own
 * urgency; everything else says so in the manifest or says nothing. */
export function toneOf(value: unknown, field?: RecordField): string {
  if (field?.type === "select")
    return (
      field.options?.find((option) => option.value === value)?.tone || "neutral"
    );
  if (field?.type !== "date") return "neutral";
  const bucket = dueBucket(text(value));
  return bucket === "Overdue"
    ? "danger"
    : bucket === "Today"
      ? "warning"
      : "neutral";
}

/** "2026-09-12" tells you nothing at a glance; "in 3 days" does. */
function relativeDate(value: string): string {
  if (!value) return "";
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const due = new Date(`${value}T00:00:00`);
  if (Number.isNaN(due.getTime())) return value;
  const days = Math.round((due.getTime() - today.getTime()) / 86400000);
  if (days === 0) return "Today";
  if (days === 1) return "Tomorrow";
  if (days === -1) return "Yesterday";
  if (days < 0) return `${-days} days ago`;
  return days <= 30 ? `in ${days} days` : value;
}

export function matches(
  item: Item,
  rule: { field: string; op?: "eq" | "ne"; value: unknown },
): boolean {
  const actual = item[rule.field] ?? null;
  const wanted = rule.value ?? null;
  return rule.op === "ne" ? actual !== wanted : actual === wanted;
}

/** Sorts the way the field reads: a select by the order its options were
 * declared, a date by the calendar, text alphabetically. Blanks sort last so
 * an unscheduled task never pushes an overdue one down the list. */
export function compareRecords(
  sort: { field: string; dir: "asc" | "desc" }[],
  byId: Map<string, RecordField>,
) {
  const rank = (item: Item, key: string) => {
    const field = byId.get(key);
    const value = item[key];
    if (field?.type === "select")
      return field.options?.findIndex((option) => option.value === value) ?? -1;
    if (field?.type === "boolean") return value ? 1 : 0;
    return text(value);
  };
  return (a: Item, b: Item) => {
    for (const rule of sort) {
      const left = rank(a, rule.field);
      const right = rank(b, rule.field);
      if (left === right) continue;
      if (left === "" || left === -1) return 1;
      if (right === "" || right === -1) return -1;
      const order = left < right ? -1 : 1;
      return rule.dir === "desc" ? -order : order;
    }
    return 0;
  };
}

/** Which pile a record belongs to. Dates become relative buckets because
 * "2026-09-12" is not a useful heading. */
export function bucketOf(
  item: Item,
  field: RecordField | undefined,
  key: string,
) {
  if (key === "$project") return text(item.$project);
  if (!field) return "";
  if (field.type === "date") return dueBucket(text(item[field.id]));
  return display(item[field.id], field) || "None";
}

function dueBucket(value: string): string {
  if (!value) return "No date";
  const today = new Date().toISOString().slice(0, 10);
  if (value < today) return "Overdue";
  if (value === today) return "Today";
  const week = new Date();
  week.setDate(week.getDate() + 7);
  return value <= week.toISOString().slice(0, 10) ? "This week" : "Later";
}

const DATE_ORDER = ["Overdue", "Today", "This week", "Later", "No date"];

/** Buckets read in the order the author declared, or in calendar order for a
 * date - never alphabetically, which would put "Overdue" after "Later". */
export function orderBuckets(field?: RecordField) {
  const order =
    field?.type === "select"
      ? [...(field.options || []).map((option) => option.label), "None"]
      : field?.type === "date"
        ? DATE_ORDER
        : null;
  if (!order) return (a: string, b: string) => a.localeCompare(b);
  return (a: string, b: string) => {
    const left = order.indexOf(a);
    const right = order.indexOf(b);
    return (
      (left < 0 ? order.length : left) - (right < 0 ? order.length : right)
    );
  };
}
