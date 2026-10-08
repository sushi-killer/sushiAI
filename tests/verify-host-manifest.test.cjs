const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

test("verifyHostManifest flags a binary changed after hashing", async () => {
  const { verifyHostManifest } =
    await import("../scripts/verify-host-manifest.mjs");
  const app = fs.mkdtempSync(path.join(os.tmpdir(), "host-manifest-"));
  try {
    const host = path.join(app, "Contents", "Resources", "host");
    fs.mkdirSync(path.join(host, "t1"), { recursive: true });
    const bytes = Buffer.from("synthetic binary");
    fs.writeFileSync(path.join(host, "t1", "sushiai"), bytes);
    fs.writeFileSync(
      path.join(host, "manifest.json"),
      JSON.stringify({
        "Test t1": {
          path: "t1/sushiai",
          size: bytes.length,
          sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
        },
      }),
    );
    assert.equal(verifyHostManifest(app).ok, true);
    fs.writeFileSync(path.join(host, "t1", "sushiai"), "re-signed bytes!");
    const result = verifyHostManifest(app);
    assert.equal(result.ok, false);
    assert.equal(result.results[0].reason, "sha256 differs");
    fs.rmSync(path.join(host, "t1"), { recursive: true });
    assert.equal(verifyHostManifest(app).results[0].reason, "file missing");
  } finally {
    fs.rmSync(app, { recursive: true, force: true });
  }
});
