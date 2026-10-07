import type { Bridge } from "./types.ts";
import { normalizeRemote } from "./app/useProjectGit.ts";
import { localTarget } from "./projectPrepare.ts";
import { LOCAL_ENDPOINT } from "./daemonSessions.ts";

type SourceBridge = Pick<
  Bridge,
  "projectSourceInspect" | "projectInspect" | "projectScanSource"
>;
type Scan = Awaited<ReturnType<Bridge["projectScanSource"]>>;

export type ProjectSourceInfo = {
  branch: string;
  remote: string;
  localFound: boolean;
  variables: Scan["variables"];
  servers: Scan["servers"];
  token: string;
  install: string;
  notice: string;
};

export async function inspectProjectSource(
  bridge: SourceBridge,
  input: {
    source: "git" | "folder" | "empty";
    url: string;
    cwd: string;
    name: string;
    home: string;
    folderEndpoint: string;
    folderLocal: boolean;
  },
  current: () => boolean,
): Promise<ProjectSourceInfo | null> {
  const info: ProjectSourceInfo = {
    branch: "",
    remote: "",
    localFound: false,
    variables: [],
    servers: {},
    token: "",
    install: "",
    notice: "",
  };
  if (input.source === "empty") return current() ? info : null;

  let root = input.cwd;
  let endpoint: string | undefined = input.folderEndpoint;
  let local = input.folderLocal;
  let example: string | undefined;
  let mcp: string | undefined;
  if (input.source === "git") {
    const repository = await bridge
      .projectSourceInspect(input.url)
      .catch(() => null);
    if (!current()) return null;
    if (!repository) {
      info.notice =
        "Could not inspect repository from this Mac. You can still prepare it on the selected host.";
      return info;
    }
    info.branch = repository.branch;
    info.install = repository.install;
    example = repository.envExample;
    mcp = repository.mcp;
    root = localTarget({
      reuseFolder: false,
      cwd: input.cwd,
      home: input.home,
      name: input.name,
    });
    const checkout = await bridge
      .projectInspect(LOCAL_ENDPOINT, {
        operation: "git_remote",
        root,
      })
      .catch(() => null);
    if (!current()) return null;
    info.localFound =
      !!checkout?.remote &&
      normalizeRemote(checkout.remote) === normalizeRemote(input.url);
    endpoint = info.localFound ? LOCAL_ENDPOINT : undefined;
    local = info.localFound;
  } else {
    const repository = await bridge
      .projectInspect(input.folderEndpoint, {
        operation: "git_remote",
        root: input.cwd,
      })
      .catch(() => null);
    if (!current()) return null;
    info.remote = repository?.remote || "";
  }

  const scan = await bridge.projectScanSource({
    endpoint,
    root,
    local,
    example,
    mcp,
  });
  if (!current()) return null;
  info.variables = scan.variables;
  info.servers = scan.servers;
  info.token = scan.token;
  if (input.source === "folder") info.install = scan.install;
  return info;
}
