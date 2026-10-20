import { VAULT_EXTENSION_ID } from "./id.ts";

export type Config = { mount: string; paths: string[] };
export type NamesResult =
  { ok: true; names: string[] } | { ok: false; reason: string };

const listeners = new Set<() => void>();

/** Calls `listener` after a project's Vault section was saved, so the rows
 * that show its names read again. Returns the unsubscribe function. */
export function onProjectChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export type AddressState = { address: string; conflict: string };

export function addressGet(): Promise<AddressState> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "address.get",
  ) as Promise<AddressState>;
}

export function addressSet(address: string): Promise<AddressState> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "address.set",
    address,
  ) as Promise<AddressState>;
}

export function addressDismiss(): Promise<AddressState> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "address.dismiss",
  ) as Promise<AddressState>;
}

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

export async function projectSet(
  id: string,
  config: { mount?: string; paths: string[] } | null,
): Promise<Config | null> {
  const saved = (await window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "project.set",
    id,
    config,
  )) as Config | null;
  listeners.forEach((listener) => listener());
  return saved;
}

export function projectNames(id: string): Promise<NamesResult | null> {
  return window.bridge!.moduleInvoke(
    VAULT_EXTENSION_ID,
    "project.names",
    id,
  ) as Promise<NamesResult | null>;
}
