# Task report

One message to the owner, in the owner's language, result first.

```text
<one or two sentences: what now works, and the commit SHA (local, not pushed)>

What changed
- <user-visible behaviour, not files>

Evidence
- npm run ci: exit <code>, <n>/<n> tests
- <criterion>: <test name or reproducible step and observed result>
- screenshots: <artifacts/*.png paths the owner can open>
- review: <reviewer verdict; findings fixed / rejected with reason>
- design: <critic verdict, if a screen changed>

Assumptions I made
- <decision> - <why it was safe to assume>

Needs you
- <question> - <what it blocks and the default I would take>

Left open
- <gap, skipped check or follow-up, with the reason>
```

Omit a section that would be empty, except `Needs you`: say "nothing" when
there is nothing, so the owner knows no decision is waiting.
