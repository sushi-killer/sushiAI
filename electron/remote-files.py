"""Project inspection and explicit, conflict-checked text saves. Executed over SSH; never installed on the host."""
import hashlib
import re
import stat
import tempfile
import base64
import json
import mimetypes
import os
from pathlib import Path
import subprocess
import sys
import copy

LIMIT = 16 * 1024 * 1024
MAX_MCP_NAME = 200
MAX_PLUGIN_NAME = 300


def resolve(root, relative="."):
    base = Path(root).expanduser().resolve(strict=True)
    target = (base / relative).resolve()
    if target != base and base not in target.parents:
        raise ValueError("Path is outside the selected project")
    return base, target


def git(root, *args):
    result = subprocess.run(
        ["git", "--no-pager", "-c", "core.fsmonitor=false", "-C", str(root), *args],
        capture_output=True, timeout=15, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"},
    )
    if result.returncode:
        raise ValueError(result.stderr.decode("utf-8", "replace")[:2000])
    if len(result.stdout) > LIMIT:
        raise ValueError("Result exceeds 16 MB; select an individual file")
    return result.stdout.decode("utf-8", "replace")


HASH_RE = re.compile(r"^[0-9a-fA-F]{4,64}$")
FIELD_SEP = "\x01"


def valid_hash(value):
    return isinstance(value, str) and bool(HASH_RE.fullmatch(value))


def parse_log(output):
    commits = []
    for line in output.split("\n"):
        if not line:
            continue
        fields = line.split(FIELD_SEP)
        if len(fields) < 6:
            continue
        commit_hash, parents, author, timestamp, decoration, subject = fields[:6]
        commits.append({
            "hash": commit_hash,
            "parents": parents.split(),
            "author": author,
            "date": int(timestamp) if timestamp.isdigit() else 0,
            "refs": [ref.strip() for ref in decoration.split(",") if ref.strip()],
            "subject": subject,
        })
    return commits


def parse_diff_tree(output):
    tokens = output.split("\0")
    files = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        index += 1
        if not token:
            continue
        code = token
        if code[0] in ("R", "C"):
            previous, path = tokens[index], tokens[index + 1]
            index += 2
            files.append({"status": code, "path": path, "previous": previous})
        else:
            files.append({"status": code, "path": tokens[index]})
            index += 1
    return files


def read_json_file(filename, fallback):
    try:
        value = json.loads(filename.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return fallback
    except json.JSONDecodeError as error:
        raise ValueError(f"Could not read Claude Code configuration: {filename}") from error
    if not isinstance(value, dict):
        raise ValueError(f"Invalid JSON object in {filename}.")
    return value


def object_map(value):
    return value if isinstance(value, dict) else {}


def mcp_names(value):
    return [
        name
        for name in value
        if isinstance(name, str)
        and name.strip()
        and len(name) <= MAX_MCP_NAME
        and "\0" not in name
    ] if isinstance(value, list) else []


def mcp_source_label(source, name):
    if name.lower().startswith("claude.ai "):
        return "Claude.ai connector"
    if name == "computer-use":
        return "Claude Code built-in"
    return {
        "local": "This project · local",
        "project": "Project · .mcp.json",
        "user": "User scope",
        "saved": "Saved project choice",
    }.get(source, "Claude Code")


def remote_root(cwd):
    if (
        not isinstance(cwd, str)
        or not cwd.startswith("/")
        or any(character in cwd for character in "\r\n\0")
    ):
        raise ValueError("Choose an existing remote project folder.")
    root = Path(os.path.abspath(cwd))
    if not root.is_dir():
        raise ValueError("Choose an existing remote project folder.")
    return root


def claude_state(cwd):
    root = remote_root(cwd)
    config_file = Path.home() / ".claude.json"
    project_file = root / ".mcp.json"
    config = read_json_file(config_file, {})
    project_mcp = read_json_file(project_file, {})
    projects = object_map(config.get("projects"))
    project = projects.get(str(root), {})
    if not isinstance(project, dict):
        project = {}
    return root, config_file, config, project, project_mcp


def claude_entries(config, project, project_mcp):
    sources = {}

    def add(name, source):
        if name not in sources:
            sources[name] = source

    for name in object_map(project.get("mcpServers")):
        add(name, "local")
    for name in object_map(project_mcp.get("mcpServers")):
        add(name, "project")
    for name in object_map(config.get("mcpServers")):
        add(name, "user")
    for name in mcp_names(project.get("disabledMcpServers")):
        add(name, "saved")
    for name in mcp_names(project.get("disabledMcpjsonServers")):
        add(name, "project")

    disabled = set(mcp_names(project.get("disabledMcpServers")))
    disabled_project = set(mcp_names(project.get("disabledMcpjsonServers")))
    return [
        {
            "name": name,
            "source": source,
            "sourceLabel": mcp_source_label(source, name),
            "disabled": name in (disabled_project if source == "project" else disabled),
        }
        for name, source in sorted(sources.items(), key=lambda item: item[0].lower())
    ]


def atomic_json_write(filename, value):
    filename.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    mode = 0o600
    try:
        mode = stat.S_IMODE(filename.stat().st_mode)
    except FileNotFoundError:
        pass
    temporary = None
    try:
        fd, temporary = tempfile.mkstemp(prefix=".claude.json.sushiai-", dir=str(filename.parent))
        os.fchmod(fd, mode)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, filename)
        temporary = None
    finally:
        if temporary:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def claude_mcp(data):
    root, config_file, config, project, project_mcp = claude_state(data.get("cwd"))
    action = data.get("action")
    if action == "list":
        return {"cwd": str(root), "servers": claude_entries(config, project, project_mcp)}
    if action != "toggle":
        raise ValueError("Unsupported Claude MCP operation")
    name = data.get("name")
    if not isinstance(name, str) or not name.strip() or len(name) > MAX_MCP_NAME or "\0" in name:
        raise ValueError("Invalid MCP server name.")
    disabled = data.get("disabled")
    if not isinstance(disabled, bool):
        raise ValueError("Invalid MCP state.")
    source = data.get("source")
    if source is not None and source not in {"local", "project", "user", "saved"}:
        raise ValueError("Invalid MCP source.")
    visible = next((server for server in claude_entries(config, project, project_mcp) if server["name"] == name), None)
    project_key = (source or (visible or {}).get("source")) == "project"
    key = "disabledMcpjsonServers" if project_key else "disabledMcpServers"
    other_key = "disabledMcpServers" if project_key else "disabledMcpjsonServers"
    next_project = copy.deepcopy(project)
    current = set(mcp_names(next_project.get(key)))
    other = set(mcp_names(next_project.get(other_key)))
    if disabled:
        current.add(name)
    else:
        current.discard(name)
    other.discard(name)
    next_project[key] = sorted(current)
    next_project[other_key] = sorted(other)
    projects = dict(object_map(config.get("projects")))
    projects[str(root)] = next_project
    next_config = dict(config)
    next_config["projects"] = projects
    atomic_json_write(config_file, next_config)
    return {
        "cwd": str(root),
        "servers": claude_entries(next_config, next_project, project_mcp),
    }


def plugin_source_label(source):
    return {
        "local": "Project · local override",
        "project": "Project · shared",
        "user": "User scope",
        "installed": "Installed plugin",
    }.get(source, "Claude Code")


def plugin_entries(user, project, local, installed):
    sources = {}

    def add(name, source):
        if (
            isinstance(name, str)
            and name.strip()
            and len(name) <= MAX_PLUGIN_NAME
            and "\0" not in name
            and name not in sources
        ):
            sources[name] = source

    for name in object_map(local.get("enabledPlugins")):
        add(name, "local")
    for name in object_map(project.get("enabledPlugins")):
        add(name, "project")
    for name in object_map(user.get("enabledPlugins")):
        add(name, "user")
    for name in object_map(installed.get("plugins")):
        add(name, "installed")

    def values(settings):
        return object_map(settings.get("enabledPlugins"))

    result = []
    for name, source in sorted(sources.items(), key=lambda item: item[0].lower()):
        if source == "local":
            value = values(local).get(name)
        elif source == "project":
            value = values(project).get(name)
        elif source == "user":
            value = values(user).get(name)
        else:
            value = values(local).get(name)
            if value is None:
                value = values(project).get(name)
            if value is None:
                value = values(user).get(name)
        result.append(
            {
                "name": name,
                "source": source,
                "sourceLabel": plugin_source_label(source),
                "disabled": value is not True,
            }
        )
    return result


def claude_plugins(data):
    root = remote_root(data.get("cwd"))
    user = read_json_file(Path.home() / ".claude/settings.json", {})
    project = read_json_file(root / ".claude/settings.json", {})
    local_file = root / ".claude/settings.local.json"
    local = read_json_file(local_file, {})
    installed = read_json_file(
        Path.home() / ".claude/plugins/installed_plugins.json", {}
    )
    action = data.get("action")
    if action == "list":
        return {
            "cwd": str(root),
            "plugins": plugin_entries(user, project, local, installed),
        }
    if action != "toggle":
        raise ValueError("Unsupported Claude plugin operation")
    name = data.get("name")
    if (
        not isinstance(name, str)
        or not name.strip()
        or len(name) > MAX_PLUGIN_NAME
        or "\0" in name
    ):
        raise ValueError("Invalid Claude Code plugin name.")
    disabled = data.get("disabled")
    if not isinstance(disabled, bool):
        raise ValueError("Invalid plugin state.")
    next_local = copy.deepcopy(local)
    enabled = dict(object_map(next_local.get("enabledPlugins")))
    enabled[name] = not disabled
    next_local["enabledPlugins"] = enabled
    atomic_json_write(local_file, next_local)
    return {
        "cwd": str(root),
        "plugins": plugin_entries(user, project, next_local, installed),
    }


def inspect(data):
    operation = data["operation"]
    if operation == "terminal_attachment":
        encoded = data.get("data")
        if not isinstance(encoded, str) or len(encoded) > 28 * 1024 * 1024:
            raise ValueError("Attach files up to 20 MB")
        content = base64.b64decode(encoded, validate=True)
        if not content or len(content) > 20 * 1024 * 1024:
            raise ValueError("Choose non-empty files up to 20 MB")
        name = Path(str(data.get("name", "pasted"))).name
        name = "".join(c if c.isascii() and (c.isalnum() or c in ".- _") else "_" for c in name)[-80:] or "pasted"
        directory = Path.home() / ".cache" / "sushiai" / "attachments"
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd, filename = tempfile.mkstemp(prefix="upload-", suffix="-" + name, dir=directory)
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(content)
        except Exception:
            os.unlink(filename)
            raise
        return {"path": filename, "size": len(content)}
    if operation == "home":
        return {"home": str(Path.home()), "socket": str(Path(data.get("socket", "~/.config/herdr/herdr.sock")).expanduser())}
    if operation == "claude_mcp":
        return claude_mcp(data)
    if operation == "claude_plugins":
        return claude_plugins(data)
    base, target = resolve(data["root"], data.get("path", "."))
    if operation == "list":
        entries = []
        for item in target.iterdir():
            if item.name == ".git" or (not data.get("hidden") and item.name.startswith(".")):
                continue
            try:
                _, real = resolve(str(base), str(item.relative_to(base)))
                metadata = real.stat()
                entries.append({"name": item.name, "path": str(item.relative_to(base)), "directory": real.is_dir(), "size": metadata.st_size})
            except (OSError, ValueError):
                continue
        entries.sort(key=lambda entry: (not entry["directory"], entry["name"].lower()))
        return {"root": str(base), "entries": entries[:5000], "truncated": len(entries) > 5000}
    if operation == "read":
        if not target.is_file():
            raise ValueError("Select a regular file")
        size = target.stat().st_size
        if size > LIMIT:
            raise ValueError("File exceeds the 16 MB preview limit")
        content = target.read_bytes()
        return {"path": str(target.relative_to(base)), "size": size, "mime": mimetypes.guess_type(str(target))[0] or "application/octet-stream", "base64": base64.b64encode(content).decode("ascii"), "hash": hashlib.sha256(content).hexdigest()}
    if operation == "write":
        if not target.is_file() or target.stat().st_size > 2 * 1024 * 1024:
            raise ValueError("Editor supports existing text files up to 2 MB")
        original = target.read_bytes()
        original.decode("utf-8")
        if b"\0" in original:
            raise ValueError("Binary files cannot be edited as text")
        if hashlib.sha256(original).hexdigest() != data.get("expectedHash"):
            raise ValueError("File changed on disk. Reload it before saving; your draft has been kept.")
        content = data.get("text")
        if not isinstance(content, str):
            raise ValueError("Invalid text")
        if b"\r\n" in original and "\r\n" not in content:
            content = content.replace("\n", "\r\n")
        encoded = content.encode("utf-8")
        if len(encoded) > 2 * 1024 * 1024:
            raise ValueError("Editor limit is 2 MB")
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".sushiai-save-", delete=False) as handle:
                temporary = Path(handle.name)
                os.chmod(temporary, stat.S_IMODE(target.stat().st_mode))
                handle.write(encoded)
                handle.flush()
                os.fsync(handle.fileno())
            # Detect changes that happened while the temporary file was being written.
            if hashlib.sha256(target.read_bytes()).hexdigest() != data["expectedHash"]:
                raise ValueError("File changed on disk. Reload it before saving; your draft has been kept.")
            os.replace(temporary, target)
        finally:
            if temporary and temporary.exists():
                temporary.unlink()
        return {"hash": hashlib.sha256(encoded).hexdigest(), "size": len(encoded)}
    if operation == "git":
        root = git(base, "rev-parse", "--show-toplevel").strip()
        if Path(root).resolve() != base:
            raise ValueError("Open the repository root to inspect all changes: " + root)
        status = git(base, "status", "--porcelain=v1", "-z", "--untracked-files=all")
        chunks = status.split("\0")
        changes = []
        index = 0
        while index < len(chunks):
            chunk = chunks[index]
            index += 1
            if not chunk:
                continue
            code, name = chunk[:2], chunk[3:]
            previous = None
            if "R" in code or "C" in code:
                previous = chunks[index]
                index += 1
            changes.append({"status": code, "path": name, "previous": previous})
        try:
            branch = git(base, "symbolic-ref", "--short", "HEAD").strip()
        except ValueError:
            branch = git(base, "rev-parse", "--short", "HEAD").strip()
        return {"root": root, "branch": branch, "changes": changes}
    if operation == "diff":
        commit = data.get("commit")
        path = data.get("path", ".")
        if commit is not None:
            if not valid_hash(commit):
                raise ValueError("Invalid commit")
            # Diff explicitly against the first parent so merge commits show their
            # net changes too: plain `git show`/`diff-tree` hide merge diffs by default.
            try:
                text = git(
                    base, "diff", "--no-ext-diff", "--no-textconv", "--no-color",
                    "--unified=4", commit + "~1", commit, "--", path,
                )
            except ValueError:
                text = git(
                    base, "show", "--no-ext-diff", "--no-textconv", "--no-color",
                    "--unified=4", commit, "--", path,
                )
            return {"text": text}
        mode = data.get("mode", "working")
        args = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=4"]
        if mode == "staged":
            args.append("--cached")
        args.extend(["--", path])
        return {"text": git(base, *args)}
    if operation == "log":
        try:
            limit = max(1, min(int(data.get("limit", 400)), 5000))
        except (TypeError, ValueError):
            limit = 400
        args = [
            "log", "--topo-order", "--decorate=short",
            "--pretty=format:%H" + FIELD_SEP + "%P" + FIELD_SEP + "%an" + FIELD_SEP + "%ct" + FIELD_SEP + "%D" + FIELD_SEP + "%s",
            "-n", str(limit),
        ]
        branch = data.get("branch")
        if branch is not None:
            if (
                not isinstance(branch, str)
                or not branch
                or branch.startswith("-")
                or "\x00" in branch
                or "\n" in branch
            ):
                raise ValueError("Invalid branch")
            # Resolve the selected ref before passing it to git log. This keeps
            # the operation read-only and makes branch history independent from
            # the worktree's checked-out branch.
            git(base, "rev-parse", "--verify", branch + "^{commit}")
            args.insert(1, branch)
        elif data.get("refs") == "all":
            args.insert(1, "--all")
        try:
            head = git(base, "symbolic-ref", "--short", "HEAD").strip()
        except ValueError:
            head = None
        output = git(base, *args)
        commits = parse_log(output)
        return {"commits": commits, "head": head, "truncated": len(commits) >= limit}
    if operation == "branches":
        # One for-each-ref call covers the whole list; trackshort gives ahead /
        # behind against the upstream without a rev-list subprocess per branch.
        output = git(
            base, "for-each-ref",
            "--format=%(refname:short)" + FIELD_SEP + "%(HEAD)" + FIELD_SEP
            + "%(upstream:short)" + FIELD_SEP + "%(upstream:trackshort)" + FIELD_SEP
            + "%(committerdate:unix)" + FIELD_SEP + "%(contents:subject)",
            "--sort=-committerdate", "refs/heads",
        )
        remote_output = git(
            base,
            "for-each-ref",
            "--format=%(refname:short)" + FIELD_SEP + "%(committerdate:unix)" + FIELD_SEP + "%(contents:subject)",
            "--sort=-committerdate",
            "refs/remotes/origin",
        )
        origin_refs = {
            line.split(FIELD_SEP, 1)[0].strip()
            for line in remote_output.splitlines()
            if line.strip() and line.split(FIELD_SEP, 1)[0].strip() != "origin/HEAD"
        }
        branches = []
        local_names = set()
        for line in output.splitlines():
            fields = line.split(FIELD_SEP, 5)
            fields += [""] * (6 - len(fields))
            name, head, upstream, track, date, subject = fields
            if not name:
                continue
            local_names.add(name)
            branches.append({
                "name": name,
                "ref": name,
                "current": head.strip() == "*",
                "upstream": upstream,
                "track": track,
                "date": int(date) if date.isdigit() else 0,
                "subject": subject,
                "local": True,
                "origin": "origin/" + name if "origin/" + name in origin_refs else "",
            })
        for remote_ref in sorted(origin_refs):
            name = remote_ref.removeprefix("origin/")
            if name in local_names:
                continue
            remote_line = next(
                (line for line in remote_output.splitlines() if line.startswith(remote_ref + FIELD_SEP)),
                "",
            )
            fields = remote_line.split(FIELD_SEP, 2)
            fields += [""] * (3 - len(fields))
            _, date, subject = fields
            branches.append({
                "name": name,
                "ref": remote_ref,
                "current": False,
                "upstream": "",
                "track": "",
                "date": int(date) if date.isdigit() else 0,
                "subject": subject,
                "local": False,
                "origin": remote_ref,
                "remoteOnly": True,
            })
        return {"branches": branches}
    if operation == "checkout":
        branch = data.get("branch")
        if not isinstance(branch, str) or not branch or branch.startswith("-"):
            raise ValueError("Invalid branch")
        # Let git own ref-name validation instead of hand-rolling a regex.
        git(base, "check-ref-format", "--branch", branch)
        # Switching with uncommitted tracked changes can silently carry them onto
        # the other branch, so refuse rather than risk losing work.
        if git(base, "status", "--porcelain", "--untracked-files=no").strip():
            raise ValueError(
                "You have uncommitted changes. Commit or stash them before switching branches."
            )
        git(base, "checkout", branch)
        return {"branch": branch}
    if operation == "commit":
        commit = data.get("commit")
        if not valid_hash(commit):
            raise ValueError("Invalid commit")
        meta = git(
            base, "show", "--no-patch",
            "--pretty=format:%H" + FIELD_SEP + "%P" + FIELD_SEP + "%an" + FIELD_SEP + "%ae" + FIELD_SEP + "%ct" + FIELD_SEP + "%D" + FIELD_SEP + "%s" + FIELD_SEP + "%b",
            commit,
        )
        fields = meta.split(FIELD_SEP, 7)
        fields += [""] * (8 - len(fields))
        full_hash, parents, author, email, timestamp, decoration, subject, body = fields
        parent_list = parents.split()
        # A plain `diff-tree <commit>` hides file changes for merge commits by
        # design; diff explicitly against the first parent so merges show their
        # net changes too. Root commits (no parents) keep using --root.
        if parent_list:
            files_raw = git(base, "diff-tree", "--no-commit-id", "--name-status", "-r", "-z", parent_list[0], commit)
        else:
            files_raw = git(base, "diff-tree", "--no-commit-id", "--name-status", "-r", "--root", "-z", commit)
        return {
            "hash": full_hash,
            "parents": parent_list,
            "author": author,
            "email": email,
            "date": int(timestamp) if timestamp.isdigit() else 0,
            "refs": [ref.strip() for ref in decoration.split(",") if ref.strip()],
            "subject": subject,
            "body": body.rstrip("\n"),
            "files": parse_diff_tree(files_raw),
        }
    raise ValueError("Unsupported project operation")


if __name__ == "__main__":
    try:
        print(json.dumps({"result": inspect(json.load(sys.stdin))}))
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}))
