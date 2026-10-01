const test = require("node:test");
const assert = require("node:assert/strict");
const {
  parseEnv,
  isSecretName,
  isSecretValue,
  isSecret,
} = require("../electron/project-import.cjs");

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

test("one classifier: names that carry credentials, whatever the spelling", () => {
  for (const name of [
    "API_TOKEN",
    "SIGNING_KEY",
    "DATABASE_SECRET",
    "DB_PASSWORD",
    "SECRET_KEY_BASE",
    "TOKEN",
    "SENTRY_DSN",
    "Authorization",
    "apiKey",
    "AWS_SESSION_TOKEN",
    "TLS_PRIVATE_KEY",
  ])
    assert.equal(isSecretName(name), true, name);
  for (const name of [
    "NODE_ENV",
    "PORT",
    "DATABASE_URL",
    "KEYBOARD",
    "LOG_LEVEL",
  ])
    assert.equal(isSecretName(name), false, name);
});

test("a value that carries a credential is a secret under any name", () => {
  assert.equal(
    isSecretValue("postgres://app:invented-pass@db.example.test/app"),
    true,
  );
  assert.equal(isSecretValue("postgres://db.example.test/app"), false);
  assert.equal(
    isSecret("DATABASE_URL", "postgres://app:invented-pass@db/x"),
    true,
  );
  assert.equal(isSecret("DATABASE_URL", "postgres://db/x"), false);
});
