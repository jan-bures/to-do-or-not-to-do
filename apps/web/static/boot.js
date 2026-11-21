// Starts the web app. A module file, because the page's Content-Security-Policy allows no inline script.
// The theme is set before the wasm module loads, so a dark system never sees a light flash.
import init from "./todo-web.js";

const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
document.documentElement.dataset.theme = dark ? "dark" : "light";

init({ module_or_path: new URL("todo-web_bg.wasm", import.meta.url) }).catch((err) => {
  console.error("The app failed to start", err);
  const p = document.createElement("p");
  p.className = "fatal";
  p.textContent = "The app couldn’t start. Reload the page to try again.";
  document.body.replaceChildren(p);
});
