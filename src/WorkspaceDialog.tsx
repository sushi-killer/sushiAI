import { useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  FolderOpen,
  GitBranch,
  Lock,
  LoaderCircle,
  Play,
  Plus,
  Server,
} from "lucide-react";
import type {
  Bridge,
  ClaudeAccount,
  ConnectionProfile,
  Project,
  ProjectGitFailure,
} from "./types";
import { Tag, Toggle } from "./orchestrator/ui";
import { GitRecovery } from "./orchestrator/GitRecovery";
import { inspectProjectSource } from "./projectSource";
import { openSettings } from "./app/openSettings";
import {
  DoneRow,
  HostProgress,
  StepFooter,
  StepHead,
  type HostStep,
} from "./NewProjectParts";
import {
  localTarget,
  projectSlug,
  rememberPrepareTimes,
  repoSlug,
  tildePath,
} from "./projectPrepare";

type Source = "git" | "folder" | "empty";
type Step = "source" | "hosts" | "environment" | "creating" | "ready";
type Host = { endpoint: string; label: string; cwd: string; local: boolean };
type Variable = {
  name: string;
  value: string;
  secret: boolean;
  /** The main process holds this secret's value from the source. */
  held?: boolean;
  availableTo?: string[];
};
type HostResult = {
  state: "queued" | "working" | "ready" | "failed";
  detail: string;
  steps: HostStep[];
  path?: string;
  git?: ProjectGitFailure;
  retry?: boolean;
};
type PrepareResult = Awaited<ReturnType<Bridge["projectHostPrepare"]>>;

const SOURCES: [Source, string, typeof GitBranch][] = [
  ["git", "Git repository", GitBranch],
  ["folder", "Folder on a host", FolderOpen],
  ["empty", "Empty", Plus],
];

export function WorkspaceDialog({
  defaultCwd,
  activeEndpoint,
  localSocket,
  connectionProfiles,
  statusByEndpoint,
  onCreate,
  onStart,
  onClose,
}: {
  defaultCwd: string;
  activeEndpoint?: string;
  localSocket: string;
  connectionProfiles: ConnectionProfile[];
  statusByEndpoint: Record<string, string>;
  onCreate(
    name: string,
    cwd: string,
    starter: string,
    endpoint?: string,
    operationId?: string,
  ): Promise<boolean>;
  onStart(): void;
  onClose(): void;
}) {
  const hosts = useMemo<Host[]>(
    () => [
      ...(localSocket
        ? [
            {
              endpoint: localSocket,
              label: "This Mac",
              cwd: defaultCwd,
              local: true,
            },
          ]
        : []),
      ...connectionProfiles
        .filter((profile) => !profile.hidden)
        .map((profile) => ({
          endpoint: `ssh:${profile.id}`,
          label: profile.name || profile.host,
          cwd: `~/sushiai/project`,
          local: false,
        })),
    ],
    [localSocket, connectionProfiles, defaultCwd],
  );
  const [step, setStep] = useState<Step>("source");
  const [source, setSource] = useState<Source>("git");
  const [url, setUrl] = useState("");
  const [cwd, setCwd] = useState(defaultCwd);
  const [folderEndpoint, setFolderEndpoint] = useState(
    activeEndpoint || localSocket,
  );
  const [homeDir, setHomeDir] = useState("");
  const [name, setName] = useState("");
  const nameEdited = useRef(false);
  const [branch, setBranch] = useState("");
  const branchEdited = useRef(false);
  const [sourceNotice, setSourceNotice] = useState("");
  const [sourceRead, setSourceRead] = useState(false);
  const sourceGeneration = useRef(0);
  const [reading, setReading] = useState(false);
  const [recent, setRecent] = useState<string[]>([]);
  const [selectedHosts, setSelectedHosts] = useState<string[]>(() =>
    [activeEndpoint || localSocket].filter(Boolean),
  );
  const [variables, setVariables] = useState<Variable[]>([]);
  const [plainCount, setPlainCount] = useState(0);
  const [showPlain, setShowPlain] = useState(false);
  const [mcp, setMcp] = useState<Record<string, unknown>>({});
  const [mcpOn, setMcpOn] = useState(true);
  const [scanToken, setScanToken] = useState("");
  /** A git source already has a checkout on This Mac. */
  const [localFound, setLocalFound] = useState(false);
  const [install, setInstall] = useState("");
  const [accountId, setAccountId] = useState("");
  const [accountMenu, setAccountMenu] = useState(false);
  const [accounts, setAccounts] = useState<ClaudeAccount[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<Record<string, HostResult>>({});
  const [filling, setFilling] = useState(false);
  const [projectRef, setProjectRef] = useState("");
  const [fills, setFills] = useState<Record<string, string>>({});
  const createdProject = useRef<Project | null>(null);
  const workspaceOpened = useRef(false);
  const workspaceOpening = useRef(false);
  const retrying = useRef(false);
  const localHost = hosts.find((host) => host.local);
  const selected = hosts.filter((host) =>
    selectedHosts.includes(host.endpoint),
  );
  const localHome = homeDir || localHost?.cwd.replace(/\/[^/]*$/, "") || "~";
  /** Where the project is, or will be, on a host that is This Mac. A folder
   * chosen on another host is not a path here, so This Mac gets its own. */
  const localPathFor = (host: Host, projectName = name) =>
    localTarget({
      reuseFolder: source === "folder" && folderEndpoint === host.endpoint,
      cwd,
      home: localHome,
      name: projectName,
    });
  const accountRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    window.bridge
      ?.claudeAccountsList()
      .then((list) => {
        setAccounts(list);
        // Only an account that is ready to use (a saved token) is chosen for
        // the owner; otherwise the host's own login stays the default.
        setAccountId(
          (current) => current || list.find((item) => item.hasValue)?.id || "",
        );
      })
      .catch(() => {});
    window.bridge
      ?.projectsList()
      .then((projects) =>
        setRecent(
          projects
            .map((project) => repoSlug(project.git.url))
            .filter(Boolean)
            .slice(-2)
            .reverse(),
        ),
      )
      .catch(() => {});
    if (localSocket)
      window.bridge
        ?.projectInspect(localSocket, { operation: "home" })
        .then((info) => setHomeDir(info.home))
        .catch(() => {});
  }, [localSocket]);
  useEffect(() => {
    if (!accountMenu) return;
    const close = (event: MouseEvent) => {
      if (!accountRef.current?.contains(event.target as Node))
        setAccountMenu(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [accountMenu]);

  async function chooseFolder() {
    const host = hosts.find((item) => item.endpoint === folderEndpoint);
    if (!host?.local) return;
    const generation = sourceGeneration.current;
    const chosen = await window.bridge?.chooseDirectory();
    if (generation !== sourceGeneration.current) return;
    if (chosen && chosen !== cwd) {
      resetSource();
      setCwd(chosen);
      setUrl("");
    }
  }

  function resetSource() {
    sourceGeneration.current += 1;
    setSourceRead(false);
    setReading(false);
    setSourceNotice("");
    setVariables([]);
    setPlainCount(0);
    setShowPlain(false);
    setMcp({});
    setMcpOn(true);
    setScanToken("");
    setInstall("");
    setLocalFound(false);
    setBranch("");
    branchEdited.current = false;
    if (!nameEdited.current) setName("");
    setError("");
    setFills({});
    setFilling(false);
  }

  /** Reads what the source offers: its branch, .env.example, .mcp.json and
   * lock file. Resolves once the answers are in state. */
  async function readSource() {
    if (!window.bridge) return false;
    const generation = ++sourceGeneration.current;
    const current = () => generation === sourceGeneration.current;
    setError("");
    setSourceNotice("");
    if (source === "git" && !url.trim()) throw new Error("Enter a git URL.");
    if (source === "folder" && !cwd.trim())
      throw new Error("Choose a project folder.");
    const derived =
      source === "folder"
        ? cwd.split("/").filter(Boolean).at(-1) || "New project"
        : source === "git"
          ? url
              .split(/[/:]/)
              .filter(Boolean)
              .at(-1)
              ?.replace(/\.git$/, "") || "New project"
          : "New project";
    const label = name.trim() || derived;
    if (!name.trim()) setName(derived);
    setReading(true);
    try {
      const info = await inspectProjectSource(
        window.bridge,
        {
          source,
          url: url.trim(),
          cwd,
          name: label,
          home: localHome,
          localSocket,
          folderEndpoint,
          folderLocal: !!hosts.find((host) => host.endpoint === folderEndpoint)
            ?.local,
        },
        current,
      );
      if (!current() || !info) return false;
      const vars = info.variables.map((item) => ({
        name: item.name,
        value: item.value ?? "",
        secret: item.secret,
        held: item.held,
        availableTo: item.availableTo,
      }));
      setVariables(vars);
      setPlainCount(vars.filter((item) => !item.secret).length);
      setMcp(info.servers);
      setScanToken(info.token);
      setInstall(info.install);
      setLocalFound(info.localFound);
      setSourceNotice(info.notice);
      if (!branchEdited.current) setBranch(info.branch);
      if (source === "folder") setUrl(info.remote);
      setSourceRead(true);
      return true;
    } catch (reason) {
      if (!current()) return false;
      throw reason;
    } finally {
      if (current()) setReading(false);
    }
  }

  /** Reads the source without leaving the step, so the repository's branch
   * and name appear as soon as a URL is pasted. */
  function previewSource() {
    if (sourceRead || reading || busy) return;
    if (source !== "git" || !url.trim()) return;
    void readSource().catch((reason) =>
      setError(reason instanceof Error ? reason.message : String(reason)),
    );
  }

  async function continueFromSource() {
    try {
      if (!sourceRead && !(await readSource())) return;
      setStep("hosts");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  const patchHost = (endpoint: string, next: Partial<HostResult>) =>
    setResults((current) => ({
      ...current,
      [endpoint]: { ...current[endpoint], ...next },
    }));

  function hostSteps(host: Host, setup = install.trim()): HostStep[] {
    return [
      { label: "Checkout", state: "pending" },
      ...(setup ? [{ label: setup, state: "pending" as const }] : []),
      { label: host.local ? "Environment" : "Secrets", state: "pending" },
    ];
  }

  function failedPreparation(
    host: Host,
    prepared: Extract<PrepareResult, { ok: false }>,
    setup = install.trim(),
  ): HostResult {
    const clone = prepared.steps.find((item) => item.id === "clone");
    const setupStep = prepared.steps.find((item) => item.id === "install");
    return {
      state: "failed",
      detail: prepared.message || "Host setup failed.",
      git: prepared.git,
      retry:
        prepared.stage === "clone" &&
        prepared.status === 403 &&
        variables.some(
          (item) =>
            ["GIT_TOKEN", "GITHUB_TOKEN"].includes(item.name) &&
            (item.value || item.held),
        ),
      steps: hostSteps(host, setup).map((item, index) => {
        const step =
          index === 0 ? clone : setup && index === 1 ? setupStep : undefined;
        return {
          ...item,
          state:
            step?.state ??
            (index === 0
              ? prepared.stage === "clone"
                ? "failed"
                : "done"
              : setup && index === 1 && prepared.stage === "setup"
                ? "failed"
                : "pending"),
          ...(step?.seconds !== undefined ? { seconds: step.seconds } : {}),
        };
      }),
    };
  }

  async function openWorkspace(host: Host, target: string) {
    if (workspaceOpened.current) return true;
    if (workspaceOpening.current || !createdProject.current) return false;
    workspaceOpening.current = true;
    try {
      const project = createdProject.current;
      const launchKey = JSON.stringify([project.id, host.endpoint, target]);
      let operationId = launches.current.get(launchKey);
      if (!operationId) {
        operationId = crypto.randomUUID();
        launches.current.set(launchKey, operationId);
      }
      const created = await onCreate(
        project.name,
        target,
        "shell",
        host.local ? undefined : host.endpoint,
        operationId,
      );
      if (!created)
        throw new Error(
          "Project files were created, but the workspace could not open.",
        );
      workspaceOpened.current = true;
      return true;
    } finally {
      workspaceOpening.current = false;
    }
  }

  const creating = useRef(false);
  const launches = useRef(new Map<string, string>());
  async function createProject() {
    // A second click before the first has rendered must not start a second one.
    if (!window.bridge || busy || creating.current) return;
    creating.current = true;
    setBusy(true);
    setError("");
    setStep("creating");
    const setup = install.trim();
    const initial = (host: Host): HostResult => ({
      state: "queued",
      detail: host.cwd,
      steps: hostSteps(host, setup),
    });
    setResults(
      Object.fromEntries(selected.map((h) => [h.endpoint, initial(h)])),
    );
    try {
      const remoteUrl = source === "git" ? url.trim() : url.trim();
      const project = await window.bridge.projectsUpsert({
        name: name.trim() || "New project",
        git: {
          url: remoteUrl,
          defaultBranch: source === "folder" ? "" : branch,
        },
        env: variables.map(({ name: key, secret, availableTo }) => ({
          name: key,
          secret,
          // MCP-only secrets stay out of agent shells and installs.
          availableTo: availableTo ?? ["setup", "agent"],
        })),
        mcp: mcpOn ? { mcpServers: mcp } : {},
        importToken: scanToken || undefined,
        setup: { install, check: "" },
        sessions: {
          claudeAccount: accountId || undefined,
        },
      });
      setProjectRef(project.id);
      createdProject.current = project;
      for (const item of variables) {
        if (item.value)
          await window.bridge.projectSecretSet(
            project.id,
            item.name,
            item.value,
          );
      }
      let failed = false;
      for (const host of selected) {
        const began = Date.now();
        const seconds = () => Math.round((Date.now() - began) / 1000);
        const stepsOf = (states: HostStep["state"][], label: string) =>
          initial(host).steps.map((item, index) => ({
            ...item,
            state: states[index] ?? "pending",
            ...(index === 0 ? { label } : {}),
          }));
        patchHost(host.endpoint, {
          state: "working",
          steps: stepsOf(["working"], "Checkout"),
        });
        try {
          const reuseFolder =
            source === "folder" && folderEndpoint === host.endpoint;
          let note = "";
          let checkoutSeconds: number | undefined;
          let installSeconds: number | undefined;
          let installError = "";
          // The folder name the project was given when it was made, never one
          // derived again from the name typed here.
          const folderName = project.slug || projectSlug(name);
          let target = host.local
            ? localPathFor(host, folderName)
            : reuseFolder
              ? cwd
              : `~/sushiai/${folderName}`;
          if (
            host.local &&
            (source === "git" ||
              source === "empty" ||
              (source === "folder" && !reuseFolder))
          ) {
            if (source === "folder" && !remoteUrl)
              throw new Error(
                "This folder has no git remote to copy to This Mac.",
              );
            const made = await window.bridge.projectLocalCreate({
              url: remoteUrl,
              cwd: target,
              branch,
              empty: source === "empty",
            });
            note = made.message;
          } else if (!host.local && !reuseFolder) {
            if (source === "folder" && !remoteUrl)
              throw new Error(
                "This folder has no git remote to clone on another host.",
              );
            const prepared = await window.bridge.projectHostPrepare(
              project.id,
              host.endpoint,
            );
            if (!prepared.ok) {
              failed = true;
              patchHost(
                host.endpoint,
                failedPreparation(host, prepared, setup),
              );
              continue;
            }
            target = prepared.path;
            note = prepared.message;
            rememberPrepareTimes(project.id, host.endpoint, prepared.steps);
            for (const step of prepared.steps ?? []) {
              if (step.id === "clone") checkoutSeconds = step.seconds;
              if (step.id === "install") installSeconds = step.seconds;
            }
          }
          const label =
            note || (reuseFolder ? "Checkout found" : "Checkout ready");
          if (host.local && setup) {
            patchHost(host.endpoint, {
              steps: stepsOf(["done", "working"], label),
            });
            checkoutSeconds = seconds();
            try {
              const ran = await window.bridge.projectLocalInstall(
                project.id,
                target,
              );
              if (ran.ran) installSeconds = ran.seconds;
            } catch (reason) {
              installError =
                reason instanceof Error ? reason.message : String(reason);
            }
          }
          const done = stepsOf(["done", "done", "done"], label).map(
            (item, index, all) => ({
              ...item,
              ...(index === 0 && checkoutSeconds !== undefined
                ? { seconds: checkoutSeconds }
                : {}),
              ...(setup && index === 1
                ? {
                    ...(installSeconds !== undefined
                      ? { seconds: installSeconds }
                      : {}),
                    ...(installError ? { state: "failed" as const } : {}),
                  }
                : {}),
              ...(!setup && index === Math.min(1, all.length - 2)
                ? { seconds: checkoutSeconds ?? seconds() }
                : {}),
            }),
          );
          // A checkout that was kept as it was says so in amber.
          if (label.startsWith("Kept the checkout"))
            done[0] = { ...done[0], state: "warning" };
          // The values were sent with the prepare above.
          if (
            !host.local &&
            variables.some((item) => item.secret || item.value || item.held)
          )
            done[done.length - 1] = {
              label: "Secrets · sent while a run is going",
              state: "done",
            };
          patchHost(host.endpoint, {
            state: "ready",
            path: target,
            detail:
              installError ||
              (host.local
                ? tildePath(target, homeDir)
                : target.replace(/^.*?\/sushiai\//, "~/sushiai/")),
            steps: done,
          });
          if (!workspaceOpened.current) {
            // The first host that is ready gets the workspace; the others
            // keep going behind it.
            try {
              await openWorkspace(host, target);
            } catch (reason) {
              setError(
                reason instanceof Error ? reason.message : String(reason),
              );
            }
          }
        } catch (reason) {
          failed = true;
          patchHost(host.endpoint, {
            retry: false,
            git: undefined,
            state: "failed",
            detail: reason instanceof Error ? reason.message : String(reason),
            steps: initial(host).steps.map((item, index) => ({
              ...item,
              state: index === 0 ? "failed" : "pending",
            })),
          });
        }
      }
      if (!failed && workspaceOpened.current) setStep("ready");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setStep("environment");
    } finally {
      setBusy(false);
      creating.current = false;
    }
  }

  async function retryPreparation(
    host: Host,
    gitUrl?: string,
    useHostLogin = false,
  ) {
    const project = createdProject.current;
    if (
      !window.bridge ||
      !project ||
      busy ||
      creating.current ||
      retrying.current
    )
      return;
    retrying.current = true;
    setBusy(true);
    setError("");
    patchHost(host.endpoint, {
      retry: false,
      state: "working",
      detail: host.cwd,
      steps: hostSteps(host, project.setup.install).map((item, index) => ({
        ...item,
        state: index === 0 ? "working" : "pending",
      })),
    });
    try {
      const prepared = await window.bridge.projectHostPrepare(
        project.id,
        host.endpoint,
        useHostLogin,
        gitUrl ? { gitUrl } : undefined,
      );
      if (!prepared.ok) {
        patchHost(
          host.endpoint,
          failedPreparation(host, prepared, project.setup.install),
        );
        return;
      }
      rememberPrepareTimes(project.id, host.endpoint, prepared.steps);
      const clone = prepared.steps.find((item) => item.id === "clone");
      const setupStep = prepared.steps.find((item) => item.id === "install");
      patchHost(host.endpoint, {
        state: "ready",
        path: prepared.path,
        git: undefined,
        detail: prepared.path.replace(/^.*?\/sushiai\//, "~/sushiai/"),
        steps: hostSteps(host, project.setup.install).map(
          (item, index, all) => {
            const step =
              index === 0
                ? clone
                : project.setup.install && index === 1
                  ? setupStep
                  : undefined;
            return {
              ...item,
              state: "done",
              ...(index === 0
                ? { label: prepared.message || "Checkout ready" }
                : {}),
              ...(index === all.length - 1 &&
              variables.some(
                (variable) =>
                  variable.secret || variable.value || variable.held,
              )
                ? { label: "Secrets · sent while a run is going" }
                : {}),
              ...(step?.seconds !== undefined ? { seconds: step.seconds } : {}),
            };
          },
        ),
      });
      try {
        await openWorkspace(host, prepared.path);
        if (
          workspaceOpened.current &&
          selected.every(
            (item) =>
              item.endpoint === host.endpoint ||
              results[item.endpoint]?.state === "ready",
          )
        )
          setStep("ready");
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    } catch (reason) {
      patchHost(host.endpoint, {
        state: "failed",
        retry: useHostLogin,
        git: undefined,
        detail: reason instanceof Error ? reason.message : String(reason),
        steps: hostSteps(host, project.setup.install).map((item, index) => ({
          ...item,
          state: index === 0 ? "failed" : "pending",
        })),
      });
    } finally {
      setBusy(false);
      retrying.current = false;
    }
  }

  async function saveFills() {
    if (!window.bridge || !projectRef) return;
    for (const [key, value] of Object.entries(fills))
      if (value) await window.bridge.projectSecretSet(projectRef, key, value);
    setVariables((current) =>
      current.map((item) =>
        fills[item.name] ? { ...item, value: fills[item.name] } : item,
      ),
    );
    setFills({});
    setFilling(false);
  }

  const sourceName = name.trim() || "New project";
  const slugOf = repoSlug(url);
  const sourceSummary = [source === "git" ? sourceName : "", slugOf, branch]
    .filter(Boolean)
    .join(" · ");
  const hostsSummary = selected.map((host) => host.label).join(", ");
  const [maskedAt, setMaskedAt] = useState(-1);
  const empties = variables.filter(
    (item) => item.secret && !item.value && !item.held,
  );
  const secretsAll = variables.filter((item) => item.secret);
  const account = accounts.find((item) => item.id === accountId);
  const hostTag = (
    host: Host,
  ): { label: string; tone: "ok" | "neutral" | "danger" } => {
    if (!host.local && statusByEndpoint[host.endpoint] !== "connected")
      return { label: "offline", tone: "danger" };
    // The folder source lives on one host; a git source on This Mac may
    // already have been cloned there.
    const found =
      source === "folder"
        ? folderEndpoint === host.endpoint
        : host.local && source === "git" && localFound;
    return found
      ? { label: "found", tone: "ok" }
      : {
          label: host.local && source === "empty" ? "new" : "clones",
          tone: "neutral",
        };
  };
  /** What the host is doing now, from the step that is under way. */
  const workingLabel = (steps: HostStep[], local: boolean) => {
    // A remote host prepares in one call, so the step under way is not known.
    if (!local) return "preparing";
    const at = steps.findIndex((item) => item.state === "working");
    return at === 0 ? "checking out" : at > 0 ? "installing" : "setting up";
  };
  const pathOf = (host: Host) =>
    host.local
      ? tildePath(localPathFor(host), homeDir)
      : `~/sushiai/${projectSlug(name || "project")}`;

  if (step === "creating") {
    const entries = selected.map((host) => ({
      host,
      result: results[host.endpoint],
    }));
    const readyHosts = entries.filter((e) => e.result?.state === "ready");
    const waiting = entries.filter(
      (e) =>
        e.result && e.result.state !== "ready" && e.result.state !== "failed",
    );
    const failed = entries.filter((e) => e.result?.state === "failed");
    const target = readyHosts[0];
    return (
      <div className="np">
        <header className="np-head">
          <h2>{`Creating ${sourceName}`}</h2>
          <p>
            {busy
              ? "Continues in the background if you close this."
              : failed.length
                ? "Fix repository access here, then retry the host."
                : "Your hosts are ready."}
          </p>
        </header>
        <div className="np-cards">
          {entries.map(({ host, result }) => (
            <HostProgress
              key={host.endpoint}
              icon={
                host.local ? (
                  <FolderOpen size={14} aria-hidden />
                ) : (
                  <Server size={14} aria-hidden />
                )
              }
              name={host.label}
              path={result?.state === "failed" ? result.detail : pathOf(host)}
              tag={
                result?.state === "ready"
                  ? { label: "ready", tone: "ok" }
                  : result?.state === "failed"
                    ? { label: "failed", tone: "danger" }
                    : result?.state === "working"
                      ? {
                          label: workingLabel(result.steps, host.local),
                          tone: "warning",
                        }
                      : { label: "queued", tone: "neutral" }
              }
              steps={result?.steps || []}
              retry={
                result?.state === "failed" && !host.local
                  ? {
                      label: result.retry
                        ? `Use ${host.label}’s git login`
                        : "Retry preparation",
                      note: result.retry
                        ? "The saved token was refused. The host’s own git login may have access."
                        : undefined,
                      disabled: busy,
                      onRetry: () =>
                        void retryPreparation(
                          host,
                          !result.retry && result.git?.transport === "ssh"
                            ? result.git.url
                            : undefined,
                          result.retry,
                        ),
                    }
                  : undefined
              }
            >
              {result?.state === "failed" &&
                result.git &&
                ["auth", "network", "host-key"].includes(result.git.kind) && (
                  <GitRecovery
                    key={`${projectRef}:${host.endpoint}:${result.git.url}`}
                    projectId={projectRef}
                    endpoint={host.endpoint}
                    failure={result.git}
                    disabled={busy}
                    onRetry={(gitUrl) => void retryPreparation(host, gitUrl)}
                  />
                )}
            </HostProgress>
          ))}
        </div>
        {error && (
          <p className="np-error" role="alert">
            {error}
          </p>
        )}
        <StepFooter
          className="after-cards"
          note={
            failed.length
              ? `${failed.map((e) => e.host.label).join(", ")} failed.`
              : waiting.length && target
                ? `${target.host.label} is ready. ${waiting.map((e) => e.host.label).join(" and ")} joins when its install finishes.`
                : target
                  ? `${target.host.label} is ready.`
                  : "Setting up your hosts…"
          }
        >
          <button className="ui-button ghost" onClick={onClose}>
            Close
          </button>
          {!busy && !workspaceOpened.current && (
            <button
              className="ui-button ghost"
              onClick={() => setStep("environment")}
            >
              Edit settings
            </button>
          )}
          <button
            className="ui-button primary"
            disabled={!target || busy}
            onClick={() => {
              if (!target?.result?.path) return;
              void openWorkspace(target.host, target.result.path)
                .then((opened) => {
                  if (opened) onClose();
                })
                .catch((reason) =>
                  setError(
                    reason instanceof Error ? reason.message : String(reason),
                  ),
                );
            }}
          >
            {target ? (
              waiting.length ? (
                <LoaderCircle size={14} className="spinning" aria-hidden />
              ) : (
                <Play size={14} aria-hidden />
              )
            ) : (
              <LoaderCircle size={14} className="spinning" aria-hidden />
            )}
            {`Open on ${target?.host.label || "…"}`}
          </button>
        </StepFooter>
      </div>
    );
  }

  if (step === "ready")
    return (
      <div className="np">
        <header className="np-head ready">
          <span className="np-ok">
            <Check size={14} aria-hidden />
          </span>
          <h2>{`${sourceName} is ready`}</h2>
        </header>
        <div className="np-summary">
          {selected.map((host) => {
            const result = results[host.endpoint];
            return (
              <div className="np-sum-row" key={host.endpoint}>
                <span>{host.label}</span>
                <strong>{result?.detail}</strong>
                <Tag tone={result?.state === "failed" ? "danger" : "ok"}>
                  {result?.state === "failed" ? "failed" : "ready"}
                </Tag>
              </div>
            );
          })}
          <div className="np-sum-row">
            <span>Environment</span>
            <strong>
              {`${variables.length} variables · ${mcpOn ? Object.keys(mcp).length : 0} MCP ${Object.keys(mcp).length === 1 ? "server" : "servers"}`}
            </strong>
          </div>
          {secretsAll.length > 0 && (
            <div className="np-sum-row secrets">
              <span>Secrets</span>
              <strong>{`${empties.length} of ${secretsAll.length} empty`}</strong>
              {empties.length > 0 && (
                <button
                  className="ui-button secondary"
                  onClick={() => setFilling((open) => !open)}
                >
                  Fill now
                </button>
              )}
            </div>
          )}
          {filling && (
            <div className="np-fill">
              {empties.map((item) => (
                <label key={item.name}>
                  <span>{item.name}</span>
                  <input
                    className="np-input"
                    type="password"
                    placeholder="paste value"
                    value={fills[item.name] || ""}
                    onChange={(event) =>
                      setFills({ ...fills, [item.name]: event.target.value })
                    }
                  />
                </label>
              ))}
              <button
                className="ui-button primary"
                onClick={() => void saveFills()}
              >
                Save secrets
              </button>
            </div>
          )}
          <div className="np-sum-row">
            <span>Claude Code</span>
            <strong>{account?.label || "Default"}</strong>
          </div>
        </div>
        <StepFooter note="Change any of this later in Project settings">
          <button className="ui-button ghost" onClick={onClose}>
            Done
          </button>
          <button className="ui-button primary" onClick={onStart}>
            <Play size={14} aria-hidden /> Start a session
          </button>
        </StepFooter>
      </div>
    );

  const plainVars = variables.filter((item) => !item.secret);
  const pastSource = step === "hosts" || step === "environment";
  return (
    <div className="np">
      <header className="np-head">
        <h2>New project</h2>
      </header>
      {pastSource && sourceNotice && <p className="np-hint">{sourceNotice}</p>}
      {pastSource ? (
        <DoneRow
          title="Source"
          summary={sourceSummary}
          onEdit={() => setStep("source")}
        />
      ) : (
        <StepHead n={1} title="Source" current />
      )}
      {step === "environment" && (
        <DoneRow
          title="Hosts"
          summary={hostsSummary}
          onEdit={() => setStep("hosts")}
        />
      )}
      {step === "source" && (
        <section className="np-body">
          <div className="np-tabs" role="tablist" aria-label="Source">
            {SOURCES.map(([kind, label, Icon]) => (
              <button
                key={kind}
                role="tab"
                aria-selected={source === kind}
                className={source === kind ? "selected" : ""}
                onClick={() => {
                  if (kind !== source) {
                    resetSource();
                    setUrl("");
                  }
                  setSource(kind);
                }}
              >
                <Icon size={13} aria-hidden />
                {label}
              </button>
            ))}
          </div>
          {source === "git" && (
            <div className="np-field">
              <GitBranch size={13} aria-hidden />
              <input
                autoFocus
                aria-label="Git URL"
                placeholder="Paste a git URL — git@github.com:… or https://…"
                value={url}
                onChange={(event) => {
                  resetSource();
                  setUrl(event.target.value);
                }}
                onBlur={previewSource}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void continueFromSource();
                }}
              />
              {reading && (
                <LoaderCircle size={13} className="spinning" aria-hidden />
              )}
              {sourceRead && (
                <Tag tone="neutral" dot={false}>
                  {branch || "default branch"}
                </Tag>
              )}
            </div>
          )}
          {source === "folder" && (
            <div className="np-rows">
              <div className="np-name">
                <span>Host</span>
                <select
                  className="np-input"
                  aria-label="Host"
                  value={folderEndpoint}
                  onChange={(event) => {
                    resetSource();
                    setFolderEndpoint(event.target.value);
                    setUrl("");
                    setSelectedHosts((current) =>
                      current.includes(event.target.value)
                        ? current
                        : [...current, event.target.value],
                    );
                  }}
                >
                  {hosts.map((host) => (
                    <option key={host.endpoint} value={host.endpoint}>
                      {host.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="np-field">
                <FolderOpen size={13} aria-hidden />
                <input
                  aria-label="Folder"
                  placeholder="/Users/you/project"
                  value={cwd}
                  onChange={(event) => {
                    resetSource();
                    setCwd(event.target.value);
                    setUrl("");
                  }}
                />
                <button
                  className="ui-button ghost np-choose"
                  onClick={chooseFolder}
                  type="button"
                  disabled={
                    !hosts.find((host) => host.endpoint === folderEndpoint)
                      ?.local
                  }
                >
                  Choose…
                </button>
              </div>
            </div>
          )}
          {source === "empty" && (
            <p className="np-hint">
              {`Start with an empty git repository at ~/sushiai/${projectSlug(name || "project")}.`}
            </p>
          )}
          {(sourceRead || source !== "git") && (
            <div className="np-name">
              <span>Name</span>
              <input
                className="np-input"
                aria-label="Name"
                placeholder="Project name"
                value={name}
                onChange={(event) => {
                  nameEdited.current = true;
                  setName(event.target.value);
                }}
              />
            </div>
          )}
          {source === "git" && sourceRead && (
            <div className="np-name">
              <span>Branch</span>
              <input
                className="np-input"
                aria-label="Branch"
                placeholder="Repository default (optional)"
                value={branch}
                onChange={(event) => {
                  branchEdited.current = true;
                  setBranch(event.target.value);
                }}
              />
            </div>
          )}
          {sourceNotice && <p className="np-hint">{sourceNotice}</p>}
          {error && (
            <p className="np-error" role="alert">
              {error}
            </p>
          )}
        </section>
      )}
      {step === "hosts" && <StepHead n={2} title="Hosts" current />}
      {step === "hosts" && (
        <section className="np-body">
          <div className="np-hosts">
            {hosts.map((host) => {
              const checked = selectedHosts.includes(host.endpoint);
              const tag = hostTag(host);
              return (
                <label
                  key={host.endpoint}
                  className={`np-host${checked ? " on" : ""}`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() =>
                      setSelectedHosts((current) =>
                        checked
                          ? current.filter((item) => item !== host.endpoint)
                          : [...current, host.endpoint],
                      )
                    }
                  />
                  <span className="np-box" aria-hidden>
                    {checked && <Check size={11} strokeWidth={3} />}
                  </span>
                  <span className="np-host-name">{host.label}</span>
                  {checked && <small>{pathOf(host)}</small>}
                  {checked && <Tag tone={tag.tone}>{tag.label}</Tag>}
                </label>
              );
            })}
          </div>
          {error && (
            <p className="np-error" role="alert">
              {error}
            </p>
          )}
        </section>
      )}
      {step !== "environment" && step !== "hosts" && (
        <StepHead n={2} title="Hosts" current={false} />
      )}
      {step === "environment" && <StepHead n={3} title="Environment" current />}
      {step !== "environment" && (
        <StepHead n={3} title="Environment" current={false} />
      )}
      {step === "environment" && (
        <section className="np-body">
          <div className="np-caption">
            {`Secrets · from ${source === "git" ? ".env.example" : ".env files"}`}
          </div>
          <div className="np-table">
            {secretsAll.length === 0 && (
              <div className="np-row">
                <span className="np-row-label">
                  No secrets found in .env files
                </span>
              </div>
            )}
            {variables
              .map((item, index) => ({ item, index }))
              .filter(({ item }) => item.secret)
              .map(({ item, index }) => (
                <div className="np-row" key={`${item.name}:${index}`}>
                  <Lock size={12} aria-hidden className="np-lock" />
                  <span className="np-row-name">{item.name}</span>
                  <input
                    className="np-input"
                    type={
                      item.value && maskedAt !== index ? "text" : "password"
                    }
                    onFocus={() => setMaskedAt(index)}
                    onBlur={() => setMaskedAt(-1)}
                    aria-label={`${item.name || "New secret"} value`}
                    placeholder={
                      item.held
                        ? "kept from the source · type to replace"
                        : index === secretsAll.length - 1
                          ? "paste value or leave empty"
                          : "paste value"
                    }
                    value={
                      item.value && maskedAt !== index
                        ? `•••••••• …${item.value.slice(-4)}`
                        : item.value
                    }
                    onChange={(event) =>
                      setVariables((current) =>
                        current.map((variable, at) =>
                          at === index
                            ? { ...variable, value: event.target.value }
                            : variable,
                        ),
                      )
                    }
                  />
                </div>
              ))}
          </div>
          {plainCount > 0 && (
            <p className="np-hint">
              <span>{`${plainCount} plain variables are filled from ${source === "git" ? ".env.example" : ".env files"}`}</span>
              <span aria-hidden>·</span>
              <button
                className="np-link"
                type="button"
                onClick={() => setShowPlain((open) => !open)}
              >
                {showPlain ? "hide" : "show"}
              </button>
            </p>
          )}
          {showPlain && (
            <div className="np-plain">
              {plainVars.map((item) => (
                <span key={item.name}>
                  {item.name}
                  {item.value ? `=${item.value}` : ""}
                </span>
              ))}
            </div>
          )}
          <div className="np-caption np-caption-runs">Runs with</div>
          <div className="np-table">
            <div className="np-row">
              <span className="np-row-label">MCP servers</span>
              <span className="np-row-value">
                {Object.keys(mcp).join(", ") || "None"}
              </span>
              <Toggle
                checked={mcpOn && Object.keys(mcp).length > 0}
                disabled={!Object.keys(mcp).length}
                label="Add MCP servers to the project"
                onChange={setMcpOn}
              />
            </div>
            <div className="np-row">
              <span className="np-row-label">Install</span>
              <input
                className="np-input"
                aria-label="Install command"
                value={install}
                placeholder="Install command (optional)"
                onChange={(event) => setInstall(event.target.value)}
              />
            </div>
            <div className="np-row">
              <span className="np-row-label">Claude Code</span>
              <span className="np-row-hint">default for new sessions</span>
              <div className="np-select-wrap" ref={accountRef}>
                <button
                  type="button"
                  className={`np-select${accountMenu ? " open" : ""}`}
                  aria-haspopup="menu"
                  aria-expanded={accountMenu}
                  aria-label={`Claude Code account: ${account?.label || "Default"}`}
                  onClick={() => setAccountMenu((open) => !open)}
                >
                  <span>{account?.label || "Default"}</span>
                  <ChevronDown size={12} aria-hidden />
                </button>
                {accountMenu && (
                  <div
                    className="np-menu"
                    role="menu"
                    aria-label="Claude Code account"
                    onKeyDown={(event) => {
                      if (event.key === "Escape") {
                        event.stopPropagation();
                        setAccountMenu(false);
                      }
                    }}
                  >
                    {(accounts.length
                      ? accounts
                      : [
                          {
                            id: "",
                            label: "Default",
                            kind: "default",
                            hasValue: false,
                          } as Pick<
                            ClaudeAccount,
                            "id" | "label" | "hasValue"
                          > & { kind: string },
                        ]
                    ).map((item) => (
                      <button
                        key={item.id || "default"}
                        role="menuitemradio"
                        aria-checked={accountId === item.id}
                        className="np-menu-item"
                        onClick={() => {
                          // Choosing the chosen account again goes back to
                          // the host's own login.
                          setAccountId(accountId === item.id ? "" : item.id);
                          setAccountMenu(false);
                        }}
                      >
                        <span>
                          <strong>{item.label}</strong>
                          <small
                            className={
                              item.kind === "subscription" && !item.hasValue
                                ? "is-warning"
                                : undefined
                            }
                          >
                            {item.id === ""
                              ? "the signed-in account on each host"
                              : item.kind === "subscription"
                                ? item.hasValue
                                  ? `subscription · on ${selected.map((host) => (host.local ? "This Mac" : host.label)).join(" and ")}`
                                  : `subscription · ${
                                      selected
                                        .filter((host) => !host.local)
                                        .map((host) => host.label)
                                        .join(", ") || "this host"
                                    } logs in on first run`
                                : "API key from Providers"}
                          </small>
                        </span>
                        {accountId === item.id && (
                          <Check size={12} aria-hidden />
                        )}
                      </button>
                    ))}
                    <div className="np-menu-rule" />
                    <button
                      role="menuitem"
                      className="np-menu-add"
                      onClick={() => openSettings("providers")}
                    >
                      <Plus size={13} aria-hidden /> Add a subscription · claude
                      setup-token
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
          {error && (
            <p className="np-error" role="alert">
              {error}
            </p>
          )}
        </section>
      )}
      {step === "source" && (
        <StepFooter
          className="after-steps"
          note={
            sourceRead || source !== "git" ? (
              source === "folder" ? (
                "Reads repository settings from this folder"
              ) : (
                "Read from the repo: branch, .env.example, .mcp.json, lock file"
              )
            ) : recent.length > 0 ? (
              <>
                Recent:{" "}
                {recent.map((item, index) => (
                  <span key={item}>
                    {index > 0 && " · "}
                    <button
                      type="button"
                      className="np-link muted"
                      onClick={() => {
                        resetSource();
                        setUrl(`git@github.com:${item}.git`);
                      }}
                    >
                      {item}
                    </button>
                  </span>
                ))}
              </>
            ) : (
              "Reads branch, .env.example, .mcp.json and lock file"
            )
          }
        >
          <button className="ui-button ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="ui-button primary"
            disabled={reading || (source === "git" && !url.trim()) || busy}
            onClick={() => void continueFromSource()}
          >
            Continue
          </button>
        </StepFooter>
      )}
      {step === "hosts" && (
        <StepFooter
          className="after-hosts"
          note={`Creates on ${hostsSummary.replace(/, ([^,]*)$/, " and $1") || "no hosts selected"}`}
        >
          <button className="ui-button ghost" onClick={() => setStep("source")}>
            Back
          </button>
          <button
            className="ui-button primary"
            disabled={!selected.length}
            onClick={() => setStep("environment")}
          >
            Continue
          </button>
        </StepFooter>
      )}
      {step === "environment" && (
        <StepFooter
          className="after-table"
          note={`${empties.length} secrets empty — fill them now or later`}
        >
          <button className="ui-button ghost" onClick={() => setStep("hosts")}>
            Back
          </button>
          <button
            className="ui-button primary"
            disabled={busy}
            onClick={() => void createProject()}
          >
            {busy ? "Creating…" : "Create project"}
          </button>
        </StepFooter>
      )}
    </div>
  );
}
