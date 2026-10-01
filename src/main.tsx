import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles/tokens.css";
import "./styles.css";
import "./styles/components/extensions.css";
import "./styles/components/project.css";
import "./styles/components/picker.css";
import "./styles/components/new-project.css";
createRoot(document.getElementById("root")!).render(<App />);
