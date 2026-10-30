import "./styles/fonts.css";
import "../../../../design-system/tokens.css";
import "@xterm/xterm/css/xterm.css";
import "./styles/app.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import {
  createOfflineBridge,
  createTauriBridge,
  runtimeAvailable,
} from "./lib/bridge";
import { Store } from "./lib/store";

const root = document.querySelector("#root");
if (!root) throw new Error("Lazoo desktop root is missing");

const bridge = runtimeAvailable()
  ? createTauriBridge()
  : import.meta.env.DEV && window.location.search.includes("mock")
    ? (await import("./lib/devBridge")).createDevBridge()
    : createOfflineBridge();
const store = new Store(bridge);

createRoot(root).render(
  <StrictMode>
    <App store={store} bridge={bridge} />
  </StrictMode>,
);
