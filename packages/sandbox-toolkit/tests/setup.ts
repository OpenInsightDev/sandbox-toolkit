import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

// Routes the server build through the `rust:build` task so the binary the
// integration tests spawn is the same artifact the task graph produces and
// caches, instead of every test file shelling out to `cargo build` in parallel.
const repoRoot = resolve(import.meta.dirname, "../../..");

export default (): void => {
  execFileSync("vp", ["run", "sandbox-toolkit-workspace#rust:build"], {
    cwd: repoRoot,
    stdio: "inherit",
  });
};
