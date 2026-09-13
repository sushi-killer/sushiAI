import {
  mkdtemp,
  mkdir,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { Connections, run } from "../electron/connections.cjs";
import { ActivityStore } from "../electron/agents/activity-store.cjs";
import { ExtensionRegistry } from "../src/extensions/registry.ts";

const root = process.cwd();
const rounds = Number(process.env.PERF_ROUNDS || 30);
const warmups = Number(process.env.PERF_WARMUPS || 3);

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[
    Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)
  ];
}

function summarize(label, samples) {
  return {
    label,
    rounds: samples.length,
    medianMs: Number(median(samples).toFixed(2)),
    p95Ms: Number(percentile(samples, 0.95).toFixed(2)),
    samplesMs: samples.map((sample) => Number(sample.toFixed(2))),
  };
}

async function measureVariants(variants) {
  for (let index = 0; index < warmups; index += 1) {
    for (const variant of variants) await variant.task();
  }
  const samples = new Map(variants.map(({ label }) => [label, []]));
  for (let round = 0; round < rounds; round += 1) {
    const order = round % 2 ? [...variants].reverse() : variants;
    for (const variant of order) {
      const started = performance.now();
      await variant.task();
      samples.get(variant.label).push(performance.now() - started);
    }
  }
  return variants.map(({ label }) => summarize(label, samples.get(label)));
}

function activityEntries(count) {
  return Array.from({ length: count }, (_, index) => ({
    id: `activity-${index}`,
    title: "Performance sample",
    createdAt: index,
    read: index % 2 === 0,
  }));
}

async function legacyActivityBurst(file) {
  for (let count = 1; count <= 100; count += 1) {
    const data = JSON.stringify(activityEntries(count));
    const temporary = `${file}.${count}.tmp`;
    await mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
    await writeFile(temporary, data, { mode: 0o600, flag: "wx" });
    await rename(temporary, file);
  }
}

function createHerdrFixture() {
  const workspaces = Array.from({ length: 100 }, (_, workspaceIndex) => ({
    workspace_id: `workspace-${workspaceIndex}`,
    label: `Workspace ${workspaceIndex}`,
  }));
  const panes = [];
  const current = workspaces.map((workspace) => ({
    id: `herdr:local:${workspace.workspace_id}`,
    name: workspace.label,
    cwd: "/tmp",
    herdrId: workspace.workspace_id,
    connection: "local",
    panels: Array.from({ length: 100 }, (_, panelIndex) => ({
      id: `herdr:local:${workspace.workspace_id}:pane-${panelIndex}`,
      kind: "terminal",
      title: "zsh",
      herdrId: `pane-${panelIndex}`,
      status: "idle",
    })),
    layout: null,
  }));
  for (const workspace of workspaces) {
    for (let paneIndex = 0; paneIndex < 1000; paneIndex += 1) {
      panes.push({
        pane_id: `pane-${paneIndex}`,
        workspace_id: workspace.workspace_id,
        agent_status: "idle",
      });
    }
  }
  return { current, snapshot: { workspaces, panes } };
}

function legacyHerdrMatch({ current, snapshot }) {
  let matched = 0;
  for (const workspace of current) {
    const panes = snapshot.panes.filter(
      (pane) => pane.workspace_id === workspace.herdrId,
    );
    for (const pane of panes) {
      if (workspace.panels.find((panel) => panel.herdrId === pane.pane_id))
        matched += 1;
    }
  }
  return matched;
}

function indexedHerdrMatch({ current, snapshot }) {
  const panesByWorkspace = new Map();
  for (const pane of snapshot.panes) {
    const panes = panesByWorkspace.get(pane.workspace_id) || [];
    panes.push(pane);
    panesByWorkspace.set(pane.workspace_id, panes);
  }
  let matched = 0;
  for (const workspace of current) {
    const panelsByHerdr = new Map(
      workspace.panels.map((panel) => [panel.herdrId, panel]),
    );
    for (const pane of panesByWorkspace.get(workspace.herdrId) || []) {
      if (panelsByHerdr.has(pane.pane_id)) matched += 1;
    }
  }
  return matched;
}

function createExtensionSnapshot() {
  const extensions = Array.from({ length: 20 }, (_, extensionIndex) => {
    const extensionId = `user.extension-${extensionIndex}`;
    return {
      manifest: {
        id: extensionId,
        name: `Extension ${extensionIndex}`,
        version: "1.0.0",
        apiVersion: 1,
        scope: "app",
        source: {
          kind: "npm",
          package: `@sushiai/extension-${extensionIndex}`,
          version: "1.0.0",
        },
        contributions: {
          surfaces: [],
          navigation: [],
          actions: [],
          commands: [],
        },
      },
      status: "active",
    };
  });
  const surfaces = Array.from({ length: 100 }, (_, surfaceIndex) => {
    const extensionId = `user.extension-${surfaceIndex % 20}`;
    return {
      id: `surface-${surfaceIndex}`,
      extensionId,
      title: `Surface ${surfaceIndex}`,
      allowedHosts: ["workspace.pane", "workspace.tab"],
      defaultHost: "workspace.pane",
      instancePolicy: "multiple",
      stateVersion: 1,
      view: {
        kind: "declarative",
        schemaVersion: 1,
        document: {
          kind: "collection",
          title: `Collection ${surfaceIndex}`,
          items: [],
        },
      },
    };
  });
  return {
    schemaVersion: 2,
    version: 1,
    extensions,
    surfaces,
    navigation: [],
    actions: [],
    commands: [],
  };
}

async function main() {
  const connectionData = await mkdtemp(path.join(tmpdir(), "sushiai-perf-"));
  const activityData = await mkdtemp(path.join(tmpdir(), "sushiai-perf-"));
  const connections = new Connections(connectionData);
  await connections.init();
  const inspectorSource = await readFile(
    path.join(root, "electron", "remote-files.py"),
    "utf8",
  );
  const inspect = (operation, extra = {}) =>
    connections.inspect(null, { operation, root, ...extra });
  const oneShotInspect = async (operation, extra = {}) => {
    const output = await run(
      "/usr/bin/python3",
      ["-c", inspectorSource],
      JSON.stringify({ operation, root, ...extra }),
    );
    const envelope = JSON.parse(output);
    if (!envelope || envelope.error || !Object.hasOwn(envelope, "result"))
      throw new Error(
        envelope?.error || "One-shot inspection returned no result.",
      );
    return envelope.result;
  };
  const results = [];

  try {
    results.push(
      ...(await measureVariants([
        {
          label: "git-baseline-one-shot-log-plus-branches",
          task: async () => {
            await Promise.all([
              oneShotInspect("log", { refs: "all", limit: 400 }),
              oneShotInspect("branches"),
            ]);
          },
        },
        {
          label: "git-worker-warm-overview",
          task: () => inspect("git_overview", { refs: "all", limit: 400 }),
        },
        {
          label: "git-baseline-one-shot-overview",
          task: () =>
            oneShotInspect("git_overview", { refs: "all", limit: 400 }),
        },
      ])),
    );

    results.push(
      ...(await measureVariants([
        {
          label: "git-worker-cold-overview",
          task: async () => {
            const coldData = await mkdtemp(path.join(connectionData, "cold-"));
            const cold = new Connections(coldData);
            await cold.init();
            try {
              await cold.inspect(null, {
                operation: "git_overview",
                root,
                refs: "all",
                limit: 400,
              });
            } finally {
              await cold.close();
              await rm(coldData, { recursive: true, force: true });
            }
          },
        },
        {
          label: "git-worker-reused-overview",
          task: () => inspect("git_overview", { refs: "all", limit: 400 }),
        },
      ])),
    );

    results.push(
      ...(await measureVariants([
        {
          label: "activity-baseline-write-every-save",
          task: async () => {
            const directory = await mkdtemp(
              path.join(activityData, "baseline-"),
            );
            try {
              await legacyActivityBurst(path.join(directory, "activity.json"));
            } finally {
              await rm(directory, { recursive: true, force: true });
            }
          },
        },
        {
          label: "activity-optimized-coalesced",
          task: async () => {
            const directory = await mkdtemp(
              path.join(activityData, "optimized-"),
            );
            try {
              const store = new ActivityStore(
                path.join(directory, "activity.json"),
              );
              for (let count = 1; count <= 100; count += 1)
                store.save(activityEntries(count));
              await store.close();
            } finally {
              await rm(directory, { recursive: true, force: true });
            }
          },
        },
      ])),
    );

    const herdrFixture = createHerdrFixture();
    results.push(
      ...(await measureVariants([
        {
          label: "herdr-baseline-linear-match",
          task: () => legacyHerdrMatch(herdrFixture),
        },
        {
          label: "herdr-optimized-indexed-match",
          task: () => indexedHerdrMatch(herdrFixture),
        },
      ])),
    );

    const extensionSnapshot = createExtensionSnapshot();
    const extensionRegistry = new ExtensionRegistry();
    extensionRegistry.applySnapshot(structuredClone(extensionSnapshot));
    const extensionPanel = extensionRegistry.createPanel(
      "user.extension-7",
      "surface-47",
      "extension-panel-47",
    );
    results.push(
      ...(await measureVariants([
        {
          label: "extension-registry-apply-100-surfaces",
          task: () => {
            const registry = new ExtensionRegistry();
            registry.applySnapshot(structuredClone(extensionSnapshot));
          },
        },
        {
          label: "extension-registry-surface-lookup",
          task: () => {
            if (!extensionRegistry.resolveSurface(extensionPanel))
              throw new Error("Extension surface lookup unexpectedly failed.");
          },
        },
      ])),
    );

    const payload = {
      root,
      rounds,
      warmups,
      workerStarts: connections.inspectionWorkers.get("local")?.starts || 0,
      results,
    };
    const rendered = JSON.stringify(payload, null, 2);
    console.log(rendered);
    if (process.env.PERF_OUTPUT)
      await writeFile(process.env.PERF_OUTPUT, `${rendered}\n`);
  } finally {
    await connections.close();
    await rm(connectionData, { recursive: true, force: true });
    await rm(activityData, { recursive: true, force: true });
  }
}

await main();
