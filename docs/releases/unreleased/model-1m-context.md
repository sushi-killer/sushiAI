## Model providers

- A model profile's context window now reaches Claude Code the same way whatever the model. A million-token window launches the model id with the `[1m]` suffix — the only lever that makes Claude Code use a million-token window and send the matching `context-1m-2025-08-07` beta — and any smaller window (128K, 200K, 256K, 512K) is declared, which used to happen only for a non-Claude model. Before this, a non-Claude model at 1M was launched with a bare id and a declared window, so the session compacted at an assumed 200K.
