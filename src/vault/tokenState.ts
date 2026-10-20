import type { TokenStatus } from "./client";

const date = (ms: number) =>
  new Date(ms).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });

/** One line for the Settings tab: what is known of the saved token. */
export function tokenStateText(status: TokenStatus): string {
  switch (status.state) {
    case "expired":
      return "Expired. Save a new token.";
    case "unreadable":
      return "Cannot be read. Save the token again.";
    case "valid":
      if (status.expiresAt === null) return "Saved, never expires";
      return status.autoRenews
        ? `Renews automatically, valid until ${date(status.expiresAt)}`
        : `Valid until ${date(status.expiresAt)}, does not renew`;
    case "unreachable":
      return status.expiresAt === null
        ? "Saved, Vault is unreachable"
        : `Vault is unreachable, valid until ${date(status.expiresAt)}`;
    default:
      return "Saved";
  }
}
