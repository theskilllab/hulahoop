/**
 * The Pi provider each per-provider connector entry signs in, keyed by entry id.
 * Mirrors `pi_provider` on the server's entries (`crates/server/src/connectors.rs`),
 * which `Connection` does not expose; keep the two in step. `models` names the
 * provider's models for the Connectors summary.
 */
export const PI_PROVIDERS: Record<string, { provider: string; models: string }> = {
  "claude-sub": { provider: "anthropic", models: "Claude" },
  "claude-api": { provider: "anthropic", models: "Claude" },
  "codex-sub": { provider: "openai-codex", models: "OpenAI Codex" },
  "opencode-go": { provider: "opencode-go", models: "OpenCode Go" },
};

/** The stored Pi login: serves whichever providers its pasted auth.json holds. */
export const PI_LOGIN = "pi";

/** The provider part of a Pi model id (`anthropic/claude-sonnet-4` → `anthropic`). */
export const providerOf = (model: string): string | null =>
  model.includes("/") ? model.split("/")[0] : null;

/**
 * Whether a Pi agent on `model` would use connector entry `id` if chosen. The Pi
 * login serves any model; a per-provider entry only its own provider's models.
 */
export function servesPiModel(id: string, model: string): boolean {
  if (id === PI_LOGIN) return true;
  const provider = providerOf(model);
  return provider !== null && PI_PROVIDERS[id]?.provider === provider;
}
