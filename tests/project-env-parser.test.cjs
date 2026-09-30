const test = require("node:test");
const assert = require("node:assert/strict");
const {
  displayedEnvValue,
  parseEnv,
  isSecretEnvName,
} = require("../src/projectEnv.ts");

test("parses export prefixes, comments, quoted values, multiline strings and equals signs", () => {
  const parsed = parseEnv(`
# ignored
export SIMPLE=value # note
SINGLE='two words'
DOUBLE="two words and \\"quotes\\""
MULTI="first line
second line"
URL=https://example.test/path?a=b=c
`);
  assert.deepEqual(parsed, [
    { name: "SIMPLE", value: "value" },
    { name: "SINGLE", value: "two words" },
    { name: "DOUBLE", value: 'two words and "quotes"' },
    { name: "MULTI", value: "first line\nsecond line" },
    { name: "URL", value: "https://example.test/path?a=b=c" },
  ]);
});

test("marks token, key and secret suffixes", () => {
  assert.equal(isSecretEnvName("API_TOKEN"), true);
  assert.equal(isSecretEnvName("SIGNING_KEY"), true);
  assert.equal(isSecretEnvName("DATABASE_SECRET"), true);
  assert.equal(isSecretEnvName("TOKEN_NAME"), false);
});

test("masks imported secrets in the review display", () => {
  const secret = "invented-secret-value";
  const shown = displayedEnvValue(secret, isSecretEnvName("API_TOKEN"));
  assert.match(shown, /•/);
  assert.equal(shown.includes(secret), false);
  assert.equal(displayedEnvValue("plain", false), "plain");
});
