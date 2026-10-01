import { wrapBridge } from "./bridge";

if (window.nativeBridge) window.bridge = wrapBridge(window.nativeBridge);
