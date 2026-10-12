const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/autonomy.ts");
const repo = "/repo";
const now = new Date(2026, 9, 12, 6, 0).getTime();
const until = new Date(2026, 9, 12, 9, 5).getTime();
const settings = {
  autopilot: true,
  autoAnswer: true,
  waitRemindEveryHours: 4,
  waitDefaultAfterHours: 24,
};

function task(overrides = {}) {
  return { repo, status: "queued", pausedUntil: until, ...overrides };
}

test("autonomySummary is absent without settings, even during a pause", async () => {
  const { autonomySummary } = await model;
  assert.equal(autonomySummary(null, [task()], repo, now), null);
});

test("autonomySummary keeps the four settings in order for every on/off combination", async () => {
  const { autonomySummary } = await model;
  for (const autopilot of [false, true]) {
    for (const autoAnswer of [false, true]) {
      for (const reminder of [0, 4]) {
        for (const defaultAnswer of [0, 24]) {
          assert.deepEqual(
            autonomySummary(
              {
                autopilot,
                autoAnswer,
                waitRemindEveryHours: reminder,
                waitDefaultAfterHours: defaultAnswer,
              },
              [],
              repo,
              now,
            ),
            {
              items: [
                {
                  label: autopilot ? "Autopilot on" : "Autopilot off",
                  on: autopilot,
                },
                {
                  label: autoAnswer ? "Auto-answer on" : "Auto-answer off",
                  on: autoAnswer,
                },
                {
                  label: reminder ? "Reminder every 4 h" : "Reminder off",
                  on: reminder > 0,
                },
                {
                  label: defaultAnswer
                    ? "Default answer after 24 h"
                    : "Default answer off",
                  on: defaultAnswer > 0,
                },
              ],
            },
          );
        }
      }
    }
  }
});

test("autonomySummary reads the configured number of hours", async () => {
  const { autonomySummary } = await model;
  const summary = autonomySummary(
    { ...settings, waitRemindEveryHours: 2, waitDefaultAfterHours: 12 },
    [],
    repo,
    now,
  );
  assert.deepEqual(summary.items.slice(2), [
    { label: "Reminder every 2 h", on: true },
    { label: "Default answer after 12 h", on: true },
  ]);
});

test("autonomySummary treats missing older-daemon hour settings as off independently", async () => {
  const { autonomySummary } = await model;
  const older = { autopilot: false, autoAnswer: true };
  assert.deepEqual(autonomySummary(older, [], repo, now).items.slice(2), [
    { label: "Reminder off", on: false },
    { label: "Default answer off", on: false },
  ]);
  assert.deepEqual(
    autonomySummary(
      { ...older, waitRemindEveryHours: 4 },
      [],
      repo,
      now,
    ).items.slice(2),
    [
      { label: "Reminder every 4 h", on: true },
      { label: "Default answer off", on: false },
    ],
  );
  assert.deepEqual(
    autonomySummary(
      { ...older, waitDefaultAfterHours: 24 },
      [],
      repo,
      now,
    ).items.slice(2),
    [
      { label: "Reminder off", on: false },
      { label: "Default answer after 24 h", on: true },
    ],
  );
});

test("autonomySummary shows a future pause using local HH:MM", async () => {
  const { autonomySummary } = await model;
  assert.deepEqual(autonomySummary(settings, [task()], repo, now).pause, {
    until,
    label: "Usage-limit pause until 09:05",
  });
});

test("autonomySummary ignores expired, exactly-ended and absent pauses", async () => {
  const { autonomySummary } = await model;
  for (const pausedUntil of [now - 1, now, undefined]) {
    const summary = autonomySummary(
      settings,
      [task({ pausedUntil })],
      repo,
      now,
    );
    assert.equal(Object.hasOwn(summary, "pause"), false);
  }
});

test("autonomySummary ignores a future pause in another repo", async () => {
  const { autonomySummary } = await model;
  const summary = autonomySummary(
    settings,
    [task({ repo: "/other" })],
    repo,
    now,
  );
  assert.equal(Object.hasOwn(summary, "pause"), false);
});

test("autonomySummary counts a pause regardless of task status, including queued and stopped", async () => {
  const { autonomySummary } = await model;
  for (const status of [
    "drafting",
    "queued",
    "running",
    "waiting",
    "landing",
    "done",
    "stopped",
    "failed",
  ]) {
    assert.equal(
      autonomySummary(settings, [task({ status })], repo, now).pause.until,
      until,
    );
  }
});

test("autonomySummary takes the latest repo pause without mutating inputs", async () => {
  const { autonomySummary } = await model;
  const latest = new Date(2026, 9, 12, 13, 45).getTime();
  const tasks = [
    task(),
    task({ status: "stopped", pausedUntil: latest }),
    task({ pausedUntil: now - 1 }),
    task({ repo: "/other", pausedUntil: latest + 3_600_000 }),
  ];
  const before = structuredClone({ settings, tasks });
  for (const ordered of [tasks, [...tasks].reverse()]) {
    assert.deepEqual(autonomySummary(settings, ordered, repo, now).pause, {
      until: latest,
      label: "Usage-limit pause until 13:45",
    });
  }
  assert.deepEqual({ settings, tasks }, before);
});
