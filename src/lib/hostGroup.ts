/** Which host (this Mac or an SSH host) owns a workspace's connection. Shared
 * by the sidebar, the Inbox and modules that need a row's host key. */
export const LOCAL_GROUP = "local";
export function groupKey(connection?: string) {
  return connection?.startsWith("ssh:") ? connection : LOCAL_GROUP;
}
