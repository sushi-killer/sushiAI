"""Project inspection and explicit, conflict-checked text saves. Executed over SSH; never installed on the host."""
import hashlib
import stat
import tempfile
import base64
import json
import mimetypes
import os
from pathlib import Path
import subprocess
import sys

LIMIT = 16 * 1024 * 1024


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
        mode = data.get("mode", "working")
        args = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=4"]
        if mode == "staged":
            args.append("--cached")
        args.extend(["--", data.get("path", ".")])
        return {"text": git(base, *args)}
    raise ValueError("Unsupported project operation")


if __name__ == "__main__":
    try:
        print(json.dumps({"result": inspect(json.load(sys.stdin))}))
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}))
