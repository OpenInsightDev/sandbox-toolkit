import { defineConfig } from "vite-plus";

/** The vendored plugin's rules, enabled at `error` for workspace package sources. */
const antiSlopRules = {
  "anti-slop/no-array-filter-map": "error",
  "anti-slop/no-reduce-accumulator-copy": "error",
  "anti-slop/no-chained-type-assertions": "error",
  "anti-slop/no-conditional-empty-object-spread": "error",
  "anti-slop/no-known-value-widening": "error",
  "anti-slop/no-module-mocking": "error",
  "anti-slop/no-object-parameters": "error",
  "anti-slop/no-reflect-apply": "error",
  "anti-slop/no-reflect-get": "error",
  "anti-slop/no-runtime-typeof": "error",
  "anti-slop/no-shape-in-symbol-names": "error",
  "anti-slop/no-unknown-parameters": "error",
  "anti-slop/no-unknown-returns": "error",
  "anti-slop/no-unknown-type-aliases": "error",
  "anti-slop/no-unsafe-dictionary-type": "error",
  "anti-slop/no-widen-then-assert": "error",
  "anti-slop/require-readable-spacing": "error",
  "anti-slop/require-safety-comment-for-type-assertion": "error",
  "anti-slop-effect/no-manual-effect-error-tag": "error",
  "anti-slop-effect/no-manual-tag-comparison": "error",
  "anti-slop-effect/no-manual-tagged-construction": "error",
  "anti-slop-effect/no-service-constructor-imports": "error",
  "anti-slop-effect/prefer-effect-match": "error",
} as const;

// Tests build tagged fixtures and narrow values ad hoc, which the anti-slop policy
// rejects in application code. They stay in lint scope so `vp check` still
// type-checks them, with the anti-slop rule group switched off.
const antiSlopOffForTests = Object.fromEntries(
  Object.keys(antiSlopRules).map((rule) => [rule, "off"]),
) as Record<string, "off">;

// Installed agent tooling, vendored upstream sources, and the Rust workspace are
// not TypeScript application source, so lint and format stay away from them.
const ignoredPaths = [
  "crates/**",
  "references/**",
  "target/**",
  "tools/oxlint/anti-slop/**",
  ".agent/**",
  ".agents/**",
  ".claude/**",
  ".codex/**",
  ".continue/**",
  ".cursor/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".roo/**",
  ".windsurf/**",
];

export default defineConfig({
  staged: {
    "*": "vp check --fix",
  },
  // Cargo-backed tasks give the TypeScript side one place to depend on the
  // server build. `rust:build` is cached on the Rust sources and restores only
  // the final binary, which is what the integration tests spawn.
  run: {
    tasks: {
      "rust:build": {
        command: "cargo build --manifest-path crates/sandbox-toolkit/Cargo.toml",
        input: ["Cargo.toml", "Cargo.lock", ".cargo/**", "crates/**", "!crates/**/target/**"],
        output: ["target/debug/sbxtkt"],
      },
      "rust:bindings": {
        command:
          "cargo test --manifest-path crates/sandbox-toolkit/Cargo.toml export_bindings",
        input: ["Cargo.toml", "Cargo.lock", ".cargo/**", "crates/**", "!crates/**/target/**"],
        output: ["packages/sandbox-toolkit/src/generated/**"],
      },
      "rust:check": {
        command: "cargo clippy --workspace --all-targets",
        cache: false,
      },
      "rust:test": {
        command: "cargo test --workspace",
        cache: false,
      },
    },
  },
  lint: {
    options: {
      typeAware: true,
      typeCheck: true,
    },
    ignorePatterns: ignoredPaths,
    jsPlugins: [
      { name: "anti-slop", specifier: "./tools/oxlint/anti-slop/index.ts" },
      {
        name: "anti-slop-effect",
        specifier: "./tools/oxlint/anti-slop/effect/index.ts",
      },
    ],
    overrides: [
      {
        files: ["packages/**"],
        rules: {
          "oxc/no-accumulating-spread": "error",
          ...antiSlopRules,
        },
      },
      {
        files: ["packages/**/tests/**"],
        rules: antiSlopOffForTests,
      },
    ],
  },
  fmt: {
    // Only format workspace package sources, leaving root metadata, docs, vendored
    // trees, and the Rust workspace alone. `src/generated/**` is written by
    // `cargo test` via ts-rs; reformatting it would churn on every regeneration.
    ignorePatterns: [
      "/*",
      "!/packages",
      "packages/**/src/generated/**",
      ...ignoredPaths,
    ],
  },
});
