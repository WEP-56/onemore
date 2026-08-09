import React from "react";
import ReactDOM from "react-dom/client";
import App from "@/app/App";
import { initAppearance } from "@/lib/appearance";
import "@/styles/app.css";
import "@/styles/cc-shell.css";

initAppearance();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
