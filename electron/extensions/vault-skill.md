---
name: sushiai-vault
description: Only where sushiAI is installed. Use when a command needs a token, API key, password or other credential, or before you suggest putting a key in .env or on disk. Tells you how to check and use a value from the project's Vault without seeing it. Works for Claude Code and Codex.
---

# sushiAI Vault values

The owner keeps credentials in HashiCorp Vault. sushiAI reads them and puts
them in the environment of the sessions and tasks it starts on This Mac. You
never fetch them yourself.

## Where values come from

- The owner saves one read-only Vault token in Settings, Vault tab.
- The owner sets the Vault address, the mount and a list of paths for each
  project in the project settings, Environment tab, Vault section.
- sushiAI reads the paths in order and merges them. A later path wins over an
  earlier one. A value the owner typed by hand in the Environment tab wins over
  Vault.
- A session reads the values when it starts, and sushiAI keeps a read for up
  to 5 minutes. A task from the orchestrator gets the copy that sushiAI sent to
  the daemon. That copy is refreshed only when the owner saves the project's
  Vault section or restarts sushiAI. For a new value, the owner does one of
  these, then starts a new session or task.
- SSH hosts never get Vault values.

## Use a value

1. Check that it is set, without printing it:
   `[ -n "$NAME" ] && echo present`
2. Use it in the command as `$NAME`.

## Never

- Never print a value. Do not echo it, log it or put it in a message.
- Never dump the environment: no `env`, `printenv`, `set` or `export -p`.
- Never read `.env` files or key files to find a value.
- Never call the Vault HTTP API or the `vault` command.
- Never write a key to `.env`, a file, the repository, a log or memory.

## A value is missing

Name the exact variable. Ask the owner to add it to Vault and to check that
the project's Vault paths include it, in sushiAI. Do not invent a workaround
or another place to store it. Then stop until the owner confirms.

## When Vault fails

If one path fails, sushiAI gives no Vault values at all. The owner sees a
notice with one of these texts.

| Notice                      | The owner does                                                     |
| --------------------------- | ------------------------------------------------------------------ |
| Vault: no token saved       | Save a token in Settings, Vault tab.                               |
| Vault: no access (403)      | Use a token that may read the paths, or change its policy.         |
| Vault: path not found (404) | Fix the mount or a path in the project's Vault section.            |
| Vault: unreachable          | Fix the address, or start Vault or the network.                    |
| Vault: timed out            | Try again later. Vault did not answer within 5 seconds.            |
| Vault: error (status)       | Check the address and the mount. The server refused the read.      |
| Vault: unreadable reply     | Check the address, the mount and that the mount is a KV v2 engine. |

The Check button in the project's Vault section lists the variable names that
sushiAI can read, never the values.
