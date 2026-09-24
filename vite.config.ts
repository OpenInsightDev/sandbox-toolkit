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
  },
  fmt: {
    // Written by `cargo test` via ts-rs; reformatting them would churn on every regeneration.
    ignorePatterns: ["src/generated/**"],
  },
});
