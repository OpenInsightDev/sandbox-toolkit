import { defineConfig } from "vite-plus";

/** The vendored plugin's rules, enabled at `error` for application code. */
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
};

// Tests build tagged fixtures and narrow values ad hoc, which the anti-slop policy
// rejects in application code. They stay in lint scope so `vp check` still
// type-checks them, with the anti-slop rule group switched off.
const antiSlopOffForTests = Object.fromEntries(
  Object.keys(antiSlopRules).map((rule) => [rule, "off"]),
) as Record<string, "off">;

// Installed agent tooling and the vendored anti-slop plugin are not application
// source, so both lint and format stay away from them.
const vendoredIgnores = [
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
  "tools/oxlint/anti-slop/**",
];

export default defineConfig({
  staged: {
    "*": "vp check --fix",
  },
  pack: {
    dts: {
      tsgo: true,
    },
    exports: true,
  },
  lint: {
    options: {
      typeAware: true,
      typeCheck: true,
    },
    // Only lint the package's own sources, leaving vendored trees alone.
    ignorePatterns: ["/*", "!/src", "!/tests", ...vendoredIgnores],
    jsPlugins: [
      { name: "anti-slop", specifier: "./tools/oxlint/anti-slop/index.ts" },
      {
        name: "anti-slop-effect",
        specifier: "./tools/oxlint/anti-slop/effect/index.ts",
      },
    ],
    rules: {
      "oxc/no-accumulating-spread": "error",
      ...antiSlopRules,
    },
    overrides: [
      {
        files: ["tests/**"],
        rules: antiSlopOffForTests,
      },
    ],
  },
  fmt: {
    // Only format the package's own sources, leaving vendored trees alone.
    // `src/generated/**` is written by `cargo test` via ts-rs; reformatting it would churn on every regeneration.
    ignorePatterns: ["/*", "!/src", "!/tests", "src/generated/**", ...vendoredIgnores],
  },
  test: {
    include: ["tests/**/*.{test,spec}.?(c|m)[jt]s?(x)"],
  },
});
