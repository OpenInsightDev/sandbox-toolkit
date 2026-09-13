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
        // The end-to-end fixtures are byte-for-byte copies of a remote server's
        // contents and must not be reformatted.
        ignorePatterns: ["tests/e2e/fixtures/**"],
        overrides: [
            {
                // Documentation code samples are authored deliberately and are not
                // compiled; keep the formatter from rewriting embedded code blocks so
                // generator syntax such as `yield*` is preserved.
                files: ["**/*.md"],
                options: { embeddedLanguageFormatting: "off" },
            },
        ],
    },
    test: {
        include: ["tests/**/*.test.ts"],
    },
});