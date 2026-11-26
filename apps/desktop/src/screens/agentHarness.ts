/** Harnesses we can run. `harness` is the provider key a connection is stored under. */
export const HARNESSES: { adapter: string; harness: string; name: string }[] = [
  { adapter: "pi", harness: "pi", name: "Pi" },
  { adapter: "claude_code", harness: "claude_code", name: "Claude Code" },
  { adapter: "fake", harness: "", name: "Fake (development)" },
];

export const harnessOf = (adapter: string) => HARNESSES.find((entry) => entry.adapter === adapter);
