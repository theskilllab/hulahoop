import { invoke } from "./api";

/// The desktop webview ignores `target="_blank"`, so a link to GitHub or a
/// sign-in page did nothing. Every such link is handed to the system browser
/// instead. In a plain browser tab there is no shell and the link opens as usual.
export function installExternalLinks(): void {
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    const link = (event.target as HTMLElement | null)?.closest<HTMLAnchorElement>("a[target='_blank']");
    if (!link || !/^https?:\/\//i.test(link.href)) return;
    if (!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) return;
    event.preventDefault();
    void invoke("open_url", { url: link.href }).catch(() => undefined);
  });
}
