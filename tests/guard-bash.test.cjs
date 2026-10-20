// scripts/hooks/guard-bash.mjs denies a merge or a push to main. A script run
// by path is scanned too, but only its own commands count: text a script
// passes to another program as quoted data or a heredoc body is not one.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const hook = path.join(__dirname, "..", "scripts", "hooks", "guard-bash.mjs");

function decision(command, cwd) {
  const run = spawnSync(process.execPath, [hook], {
    input: JSON.stringify({ tool_input: { command }, cwd }),
    encoding: "utf8",
    env: { ...process.env, SUSHIAI_GUARD_ASK: "off" },
  });
  assert.equal(run.status, 0, run.stderr);
  return run.stdout ? JSON.parse(run.stdout).hookSpecificOutput : null;
}

function scriptDecision(body) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "guard-bash-"));
  try {
    const file = path.join(dir, "script.sh");
    fs.writeFileSync(file, body);
    return decision(`bash ${file}`, dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const denied = (result) => assert.equal(result?.permissionDecision, "deny");
const allowed = (result) => assert.equal(result, null);

test("a command string still denies a merge and a push to main", () => {
  denied(decision("gh pr merge 1", os.tmpdir()));
  denied(decision("echo hi && git push origin main", os.tmpdir()));
  allowed(decision('git commit -m "docs: git push origin main"', os.tmpdir()));
  allowed(decision("echo 'a; gh pr merge 1'", os.tmpdir()));
});

test("quoted data in a script is not a command", () => {
  allowed(scriptDecision("asks 'git push origin main'\n"));
  allowed(scriptDecision(`printf '%s' "gh pr merge 1"\n`));
  allowed(scriptDecision("asks 'ok; git push origin main'\n"));
  allowed(scriptDecision(`printf '%s' "a && gh pr merge 1"\n`));
  allowed(scriptDecision("asks 'first line\ngit push origin main'\n"));
  allowed(scriptDecision(`printf '%s' "$(date) gh pr merge 1"\n`));
});

test("a heredoc body fed to a non-shell is data", () => {
  allowed(scriptDecision("cat <<'EOF'\ngit push origin main\nEOF\n"));
  allowed(
    scriptDecision("cat <<EOF > out.txt\ngh pr merge 1\nEOF\necho done\n"),
  );
});

test("a heredoc body fed to a shell is a command", () => {
  denied(scriptDecision("bash <<EOF\ngit push origin main\nEOF\n"));
  denied(scriptDecision("cat <<EOF | sh\ngh pr merge 1\nEOF\n"));
});

test("a command a script really runs is denied", () => {
  denied(scriptDecision("echo start\ngit push origin main\n"));
  denied(scriptDecision("gh pr merge 1\n"));
  denied(scriptDecision("true && git push origin main\n"));
});

test("a shell string and a command substitution are commands", () => {
  denied(scriptDecision("bash -c 'git push origin main'\n"));
  denied(scriptDecision(`sh -c "gh pr merge 1"\n`));
  denied(scriptDecision("eval 'git push origin main'\n"));
  denied(scriptDecision("echo $(git push origin main)\n"));
  denied(scriptDecision(`echo "$(gh pr merge 1)"\n`));
  denied(decision("bash -c 'git push origin main'", os.tmpdir()));
});
