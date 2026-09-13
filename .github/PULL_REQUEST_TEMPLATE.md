## What changed and why

<!-- One or two sentences. Link the task/issue if there is one. -->

## Definition of Done (see AGENTS.md)

- [ ] Branch prefix and PR title pair correctly (`feature/`→`feat: `, `fix/`→`fix: `, `chore/`→anything else) - CI's `conventions` job enforces this
- [ ] `npm run ci` is green
- [ ] Desktop smoke (`npm run test:desktop`) run if `src/app`, `src/extensions`, or `electron/` changed
- [ ] UI changes were actually looked at (screenshot or description of what was checked) - green tests don't prove a screen looks right
- [ ] A `docs/releases/unreleased/*.md` fragment added for any user-visible change (required if the title has a `!` breaking marker)
- [ ] `docs/LESSONS.md` has a new entry, or no lesson applies here
