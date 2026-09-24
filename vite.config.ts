import { defineConfig } from "vite-plus";

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
    ignorePatterns: ["/*", "!/src", "!/tests"],
  },
  fmt: {
    // Only format the package's own sources, leaving vendored trees alone.
    // `src/generated/**` is written by `cargo test` via ts-rs; reformatting it would churn on every regeneration.
    ignorePatterns: ["/*", "!/src", "!/tests", "src/generated/**"],
  },
  test: {
    include: ["tests/**/*.{test,spec}.?(c|m)[jt]s?(x)"],
  },
});
