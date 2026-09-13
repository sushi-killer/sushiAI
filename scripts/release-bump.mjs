// Pure version-bump logic, split out of release.mjs so it's unit-testable
// without invoking git/npm. This decides real version numbers - see
// tests/release-bump.test.cjs.
export function bumpFor(subject, isPre1) {
  // A `!` breaking marker maps to minor while the app stays 0.x - semver's
  // own pre-1.0 convention (anything can break in a 0.x minor; see AGENTS.md).
  if (/^[a-z]+(\([^)]+\))?!: /.test(subject)) return isPre1 ? "minor" : "major";
  if (/^feat(\([^)]+\))?: /.test(subject)) return "minor";
  if (/^fix(\([^)]+\))?: /.test(subject)) return "patch";
  return null;
}

export function computeBump(subjects, isPre1) {
  const RANK = { patch: 1, minor: 2, major: 3 };
  return subjects.reduce((best, subject) => {
    const candidate = bumpFor(subject, isPre1);
    if (!candidate) return best;
    return !best || RANK[candidate] > RANK[best] ? candidate : best;
  }, null);
}

export function bumpVersion(version, bump) {
  const [major, minor, patch] = version.split(".").map(Number);
  if (bump === "major") return `${major + 1}.0.0`;
  if (bump === "minor") return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}
