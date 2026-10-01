// The environment setup a host gets on connect: what it already has stays,
// and a Herdr server that is not running is started.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { setupHost, setupSummary } = require("../electron/host-setup.cjs");
const { makeHost } = require("./helpers/fake-host.cjs");

test("a host with every tool keeps them, and its stopped Herdr server is started", async (t) => {
  const host = await makeHost(t, {
    bin: {
      // Nothing is missing, so nothing is downloaded.
      curl: "#!/bin/sh\nexit 1\n",
      claude: "#!/bin/sh\n",
      codex: "#!/bin/sh\n",
      // A server is running once `herdr server` was run.
      herdr: `#!/bin/sh
case "$1 $2" in
  "server ") touch "$HOME/.herdr-up" ;;
  "status server") [ -e "$HOME/.herdr-up" ] && echo "status: running" || echo "status: not running" ;;
esac
`,
    },
  });
  const states = await setupHost(host.connections, host.endpoint);
  assert.deepEqual(states, {
    herdr: "present",
    claude: "present",
    codex: "present",
    server: "started",
  });
  assert.equal(setupSummary(states), "server started");
  // A second run finds it all in place.
  assert.equal(
    (await setupHost(host.connections, host.endpoint)).server,
    "running",
  );
});

test("a connection's own socket is the one the started server listens on", async (t) => {
  const host = await makeHost(t, {
    bin: {
      curl: "#!/bin/sh\nexit 1\n",
      claude: "#!/bin/sh\n",
      codex: "#!/bin/sh\n",
      herdr: `#!/bin/sh
case "$1 $2" in
  "server ") printf '%s' "\${HERDR_SOCKET_PATH:-default}" > "$HOME/socket" ;;
  "status server") [ -e "$HOME/socket" ] && echo "status: running" || echo "status: not running" ;;
esac
`,
    },
  });
  const fs = require("node:fs/promises");
  const path = require("node:path");
  await setupHost(host.connections, host.endpoint, "~/run/my herdr.sock");
  assert.equal(
    await fs.readFile(path.join(host.home, "socket"), "utf8"),
    path.join(host.home, "run/my herdr.sock"),
  );
});
