import { lazy } from "react";

// Settings screens are large and rarely opened: keep them out of the initial
// chunk. Both the settings dialog and the updates dialog load them from here.
export const ConnectionsSettings = lazy(() =>
  import("../ConnectionsSettings").then(
    ({ ConnectionsSettings: Component }) => ({ default: Component }),
  ),
);
export const ProvidersSettings = lazy(() =>
  import("../ProvidersSettings").then(({ ProvidersSettings: Component }) => ({
    default: Component,
  })),
);
export const UpdateSettings = lazy(() =>
  import("../UpdateSettings").then(({ UpdateSettings: Component }) => ({
    default: Component,
  })),
);
