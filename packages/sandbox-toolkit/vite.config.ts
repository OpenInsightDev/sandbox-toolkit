import { defineConfig } from "vite-plus";

export default defineConfig({
  pack: {
    dts: {
      tsgo: true,
    },
    exports: true,
  },
  run: {
    tasks: {
      test: {
        command: "vp test",
        // The integration tests drive the server binary the sandbox build
        // publishes; nothing else produces it.
        dependsOn: ["sandbox-toolkit-workspace#rust:build:linux"],
        cache: false,
      },
    },
  },
  test: {
    // A cold sandbox starts a container and materializes the tools it embeds;
    // this sits above the harness's own readiness timeout, so a sandbox that
    // never answers reports the harness's message rather than a bare timeout.
    testTimeout: 60_000,
  },
  lint: {
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },
  fmt: {},
});
