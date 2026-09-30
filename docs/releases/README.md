# Release fragments

Create a user facing release fragment with:

```sh
scripts/new-release-fragment.sh <slug> "<Title>"
```

The command writes this shape to `docs/releases/unreleased/<slug>.md`:

```md
## <Title>

- TODO: describe the change for users.
```

Fragments are user facing prose, written in English, with no attribution
trailers. At release time, `scripts/release.mjs` folds every
`docs/releases/unreleased/*.md` file into `docs/releases/<version>.md` and
deletes the fragments. Do not hand date fragments.

For lessons, follow the formats in [`docs/LESSONS.md`](../LESSONS.md). A
promoted index line has this form:

```md
- YYYY-MM-DD symptom → rule. `test-or-symbol`
```

An open entry uses this block:

```md
## YYYY-MM-DD — <short symptom>

Root cause: ...
Rule: ...
```
