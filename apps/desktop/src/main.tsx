import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { installDrag } from "./drag";
import "./styles.css";

// The window's title bar is an overlay on macOS: the traffic lights float over
// our first row, so that row has to make room for them.
if (/Mac/i.test(navigator.platform) || /Mac OS X/i.test(navigator.userAgent)) {
  document.documentElement.dataset.os = "mac";
}
import { applyTheme, watchSystemTheme } from "./theme";

applyTheme();
watchSystemTheme();

/**
 * A render error used to leave a white window with nothing to read: the screen is
 * the only interface, so it says what broke and keeps the app alive to try again.
 */
class Boundary extends React.Component<{ children: React.ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="boot">
          <h1>The screen hit a bug</h1>
          <p className="error">{this.state.error.message}</p>
          <pre className="log">{this.state.error.stack?.split("\n").slice(0, 8).join("\n")}</pre>
          <button className="primary" onClick={() => this.setState({ error: null })}>
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

installDrag();

// The native drop handler is off (`dragDropEnabled: false`) so the drop zones can
// read files through HTML5. A file let go anywhere else would make the webview
// navigate to it and throw away the app and every draft in it. Real drop zones
// handle the event on the way up and call `preventDefault` themselves; anywhere
// else the drag shows "no drop", and a drop that still arrives opens nothing.
window.addEventListener("dragover", (event) => {
  if (event.defaultPrevented) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
});
window.addEventListener("drop", (event) => event.preventDefault());

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Boundary>
      <App />
    </Boundary>
  </React.StrictMode>,
);
