import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import { ToastProvider } from "./components/Toast";
import { SpeedBar } from "./components/netspeed/SpeedBar";
import "./App.css";

/** The net speed bar's own window (or `?speedbar` in a browser, for the checks). */
function speedBarWindow(): boolean {
  if (new URLSearchParams(window.location.search).has("speedbar")) return true;
  try {
    return getCurrentWindow().label === "speedbar";
  } catch {
    return false;
  }
}

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

if (speedBarWindow()) {
  // Clear over the Windows taskbar; a small strip elsewhere.
  root.render(<SpeedBar transparent={navigator.userAgent.includes("Windows")} />);
} else {
  root.render(
    <React.StrictMode>
      <ToastProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </ToastProvider>
    </React.StrictMode>
  );
}
