import { createContext, useContext } from "react";
import {
  LOCAL,
  orchestratorClientFor,
  type OrchestratorClient,
} from "./client";

/** The host whose daemon the Orchestrator views below talk to. */
const HostContext = createContext<string>(LOCAL);
export const OrchestratorHostProvider = HostContext.Provider;

const clients = new Map<string, OrchestratorClient>();
function clientFor(host: string): OrchestratorClient {
  let client = clients.get(host);
  if (!client) {
    client = orchestratorClientFor(host);
    clients.set(host, client);
  }
  return client;
}

export function useOrchestratorHost(): string {
  return useContext(HostContext);
}

/** The typed client of the host in context; the same object per host. */
export function useOrchestratorClient(): OrchestratorClient {
  return clientFor(useContext(HostContext));
}
