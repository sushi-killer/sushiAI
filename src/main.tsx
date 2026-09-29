import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles/tokens.css";
import "./styles.css";
import "./styles/components/extensions.css";
import "./styles/components/orchestrator-toasts.css";
createRoot(document.getElementById("root")!).render(<App />);
