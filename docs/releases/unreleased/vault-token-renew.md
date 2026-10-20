## Vault token renewal

- sushiAI now looks at the saved Vault token when you save it, when the app starts and while it runs. A token that can be renewed is renewed before it expires, so Vault values no longer disappear after the token's time to live.
- A token that cannot be renewed (not renewable, or at its maximum lifetime) gets a notice one day before it expires. A notice also tells you when the token has expired or was refused.
- Settings, Vault shows the token state: valid until a date, renews automatically, expired, or unreadable.
- A saved token that can no longer be decrypted on this Mac is now reported as unreadable, with a notice, instead of looking like no token.
