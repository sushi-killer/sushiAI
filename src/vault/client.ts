import { VAULT_EXTENSION_ID } from "./id.ts";

export type Config = { address: string; mount: string; paths: string[] };
export type CheckResult =
  { ok: true; names: string[]; count: number } | { ok: false; reason: string };

export function tokenStatus(): Promise<{ saved: boolean }> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "token.status",
  ) as Promise<{ saved: boolean }>;
}

export function tokenSet(token: string): Promise<{ saved: true }> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "token.set",
    token,
  ) as Promise<{ saved: true }>;
}

export function tokenClear(): Promise<{ saved: false }> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "token.clear",
  ) as Promise<{ saved: false }>;
}

export function projectGet(id: string): Promise<Config | null> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "project.get",
    id,
  ) as Promise<Config | null>;
}

export function projectSet(
  id: string,
  config: { address: string; mount?: string; paths: string[] } | null,
): Promise<Config | null> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "project.set",
    id,
    config,
  ) as Promise<Config | null>;
}

export function projectCheck(id: string): Promise<CheckResult> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "project.check",
    id,
  ) as Promise<CheckResult>;
}
