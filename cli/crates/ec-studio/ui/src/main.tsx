import React from "react";
import ReactDOM from "react-dom/client";
// Brand tokens first: they map onto Carbon's --cds-* variables that index.scss then wires up.
import "./styles/edgecommons-tokens.css";
import "./index.scss";
// The context-spine shell (rail, breadcrumb, level-scoped tabs) — layout over those same tokens.
import "./shell.scss";
import { App } from "./App";

// The Studio ships dark by default (the deck's g100 shell); stamp data-theme so the brand tokens'
// dark values apply — the same mechanism edge-console uses.
document.documentElement.setAttribute("data-theme", "dark");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
