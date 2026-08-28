import "./styles/fonts.css";
import "../../../../design-system/tokens.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";

const root = document.querySelector("#root");

if (!root) {
  throw new Error("Lazoo desktop root is missing");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
