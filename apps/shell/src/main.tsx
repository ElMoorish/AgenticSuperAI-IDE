import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { bootstrapTheme } from "./bootstrap";
import { isTauri } from "./ipc";
import "./styles/tokens.css";

bootstrapTheme();

async function start() {
  // Dev-only in-memory host for plain-browser runs; removed from production builds.
  if (import.meta.env.DEV && !isTauri()) {
    const { installMockHost } = await import("./dev/mockHost");
    installMockHost();
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
void start();
