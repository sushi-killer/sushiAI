const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/extensions/records.ts");

const tone = {
  id: "tone",
  type: "select",
  label: "Tone",
  options: [
    { value: "neutral", label: "Neutral", tone: "neutral" },
    { value: "ok", label: "Ok", tone: "ok" },
    { value: "danger", label: "Danger", tone: "danger" },
  ],
};
const collected = { id: "collected", type: "date", label: "Collected" };
const fragile = { id: "fragile", type: "boolean", label: "Fragile" };
const name = { id: "name", type: "text", label: "Specimen" };

/** These read the calendar, so the calendar is held still. Without this the
 * assertions below would start failing on a date nobody chose. */
const at = (t, iso) =>
  t.mock.timers.enable({
    apis: ["Date"],
    now: new Date(`${iso}T09:00:00Z`).getTime(),
  });

test("a record is born matching the view it was typed into", async () => {
  const { seedForFilter } = await library;
  assert.deepEqual(
    seedForFilter([{ field: "shape", op: "eq", value: "cube" }], "name"),
    { shape: "cube" },
    "a view filtered to one value would otherwise hide every record added to it",
  );
  assert.deepEqual(
    seedForFilter([{ field: "catalogued", op: "ne", value: true }], "name"),
    {},
    '"not this" leaves several values open, so there is nothing to pick',
  );
  assert.deepEqual(
    seedForFilter([{ field: "name", op: "eq", value: "fixed" }], "name"),
    {},
    "the typed text wins over a filter on the same field",
  );
  assert.deepEqual(seedForFilter(undefined, "name"), {});
});

test("a field reads the way its type means it to", async (t) => {
  const { display } = await library;
  at(t, "2026-09-12");
  assert.equal(display("ok", tone), "Ok", "a select shows its label");
  assert.equal(display("elsewhere", tone), "elsewhere", "an unknown value is not hidden");
  assert.equal(display(true, fragile), "Fragile", "a flag reads as its name");
  assert.equal(display(false, fragile), "", "and says nothing when it is not set");
  assert.equal(display("2026-09-12", collected), "Today");
  assert.equal(display("2026-09-13", collected), "Tomorrow");
  assert.equal(display("2026-09-11", collected), "Yesterday");
  assert.equal(display("2026-09-05", collected), "7 days ago");
  assert.equal(display("2026-09-20", collected), "in 8 days");
  assert.equal(
    display("2027-01-01", collected),
    "2027-01-01",
    "past a month the exact date says more than the distance",
  );
  assert.equal(display("not a date", collected), "not a date");
  assert.equal(display(undefined, name), "");
});

test("a date carries its own urgency; everything else declares one", async (t) => {
  const { toneOf } = await library;
  at(t, "2026-09-12");
  assert.equal(toneOf("danger", tone), "danger", "a select says so in the manifest");
  assert.equal(toneOf("missing", tone), "neutral");
  assert.equal(toneOf("2026-09-11", collected), "danger", "overdue");
  assert.equal(toneOf("2026-09-12", collected), "warning", "today");
  assert.equal(toneOf("2026-09-30", collected), "neutral");
  assert.equal(toneOf("anything", name), "neutral", "text means nothing on its own");
});

test("a filter compares a missing value with an absent one", async () => {
  const { matches } = await library;
  const rule = (field, op, value) => ({ field, op, value });
  assert.equal(matches({ id: "a", tone: "ok" }, rule("tone", "eq", "ok")), true);
  assert.equal(matches({ id: "a", tone: "ok" }, rule("tone", "ne", "ok")), false);
  assert.equal(
    matches({ id: "a" }, rule("catalogued", "ne", true)),
    true,
    "a record that never set the field is not the value being excluded",
  );
  assert.equal(
    matches({ id: "a", catalogued: undefined }, rule("catalogued", "eq", null)),
    true,
    "unset and null are the same absence",
  );
  assert.equal(
    matches({ id: "a", tone: "ok" }, { field: "tone", value: "ok" }),
    true,
    "eq is the default",
  );
});

test("records sort the way their field reads, and blanks sort last", async () => {
  const { compareRecords } = await library;
  const byId = new Map([tone, collected, fragile, name].map((f) => [f.id, f]));
  const sorted = (sort, items) => [...items].sort(compareRecords(sort, byId)).map((i) => i.id);
  assert.deepEqual(
    sorted(
      [{ field: "tone", dir: "asc" }],
      [
        { id: "danger", tone: "danger" },
        { id: "neutral", tone: "neutral" },
        { id: "ok", tone: "ok" },
      ],
    ),
    ["neutral", "ok", "danger"],
    "a select sorts by the order its options were declared, not alphabetically",
  );
  assert.deepEqual(
    sorted(
      [{ field: "collected", dir: "asc" }],
      [
        { id: "late", collected: "2026-09-20" },
        { id: "none" },
        { id: "early", collected: "2026-09-01" },
      ],
    ),
    ["early", "late", "none"],
    "an undated record never pushes a dated one down the list",
  );
  assert.deepEqual(
    sorted(
      [{ field: "collected", dir: "desc" }],
      [
        { id: "early", collected: "2026-09-01" },
        { id: "none" },
        { id: "late", collected: "2026-09-20" },
      ],
    ),
    ["late", "early", "none"],
    "reversing the direction does not promote the blanks",
  );
  assert.deepEqual(
    sorted(
      [{ field: "fragile", dir: "desc" }],
      [
        { id: "no", fragile: false },
        { id: "yes", fragile: true },
      ],
    ),
    ["yes", "no"],
  );
  assert.deepEqual(
    sorted(
      [
        { field: "tone", dir: "asc" },
        { field: "name", dir: "asc" },
      ],
      [
        { id: "b", tone: "ok", name: "Beta" },
        { id: "a", tone: "ok", name: "Alpha" },
        { id: "c", tone: "neutral", name: "Gamma" },
      ],
    ),
    ["c", "a", "b"],
    "the second rule only decides what the first left tied",
  );
});

test("buckets are piles a person would name", async (t) => {
  const { bucketOf } = await library;
  at(t, "2026-09-12");
  assert.equal(bucketOf({ id: "a", $project: "Smart Read" }, undefined, "$project"), "Smart Read");
  assert.equal(bucketOf({ id: "a", collected: "2026-09-01" }, collected, "collected"), "Overdue");
  assert.equal(bucketOf({ id: "a", collected: "2026-09-12" }, collected, "collected"), "Today");
  assert.equal(bucketOf({ id: "a", collected: "2026-09-16" }, collected, "collected"), "This week");
  assert.equal(bucketOf({ id: "a", collected: "2026-11-01" }, collected, "collected"), "Later");
  assert.equal(bucketOf({ id: "a" }, collected, "collected"), "No date");
  assert.equal(bucketOf({ id: "a", tone: "ok" }, tone, "tone"), "Ok");
  assert.equal(
    bucketOf({ id: "a" }, tone, "tone"),
    "None",
    "a record with no value still belongs somewhere",
  );
});

test("bucket headings read in their own order, never alphabetically", async () => {
  const { orderBuckets } = await library;
  assert.deepEqual(
    ["Later", "No date", "Overdue", "This week", "Today"].sort(orderBuckets(collected)),
    ["Overdue", "Today", "This week", "Later", "No date"],
    "alphabetical order would put Overdue after Later",
  );
  assert.deepEqual(
    ["None", "Danger", "Neutral", "Ok"].sort(orderBuckets(tone)),
    ["Neutral", "Ok", "Danger", "None"],
    "a select keeps the order the manifest declared, with the empty pile last",
  );
  assert.deepEqual(
    ["Beta", "Alpha"].sort(orderBuckets(name)),
    ["Alpha", "Beta"],
    "with no declared order there is nothing to follow but the alphabet",
  );
});

test("truthy is about whether a value was set, not whether it is large", async () => {
  const { truthy, text } = await library;
  assert.equal(truthy(0), true, "zero is a value somebody chose");
  assert.equal(truthy(false), false);
  assert.equal(truthy(""), false);
  assert.equal(truthy(undefined), false);
  assert.equal(text(7), "", "a number is not text the renderer will print");
});
