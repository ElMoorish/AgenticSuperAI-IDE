import React from "react";
import ReactDOM from "react-dom/client";
import { bootstrapTheme } from "../bootstrap";
import OverlayApp from "./OverlayApp";
import "../styles/tokens.css";
import "../styles/agent.css";

bootstrapTheme();
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <OverlayApp />
  </React.StrictMode>,
);
