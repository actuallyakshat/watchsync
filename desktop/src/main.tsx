import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "@fontsource/instrument-serif";
import "./styles.css";
import { App } from "./App";
import { CallApp } from "./call/CallApp";

// The same page runs both windows: the app, and the floating call window.
const isCall = window.location.hash === "#call";
if (isCall) document.documentElement.classList.add("call-window");

createRoot(document.getElementById("root")!).render(<StrictMode>{isCall ? <CallApp /> : <App />}</StrictMode>);
