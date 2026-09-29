import { createRoot } from "react-dom/client";
import "../styles/tokens.css";
import "./mascot.css";
import { Mascot } from "./Mascot";
createRoot(document.getElementById("root")!).render(<Mascot />);
