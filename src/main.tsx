import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, initialTheme } from "./lib/theme";

// Before the first render, not in an effect. Painting light and correcting it a frame later is a
// white flash on every launch in dark mode — the one moment the window certainly has someone's
// attention.
applyTheme(initialTheme());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
