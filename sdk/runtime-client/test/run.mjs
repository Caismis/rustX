import { spawnSync } from "node:child_process";
// Accept the repository task's serial-test spelling using Node's native runner.
const args = process.argv.slice(2).filter((arg) => arg !== "--");
if (args.some((arg) => arg !== "--runInBand")) throw new Error("Unknown test option");
const result = spawnSync(process.execPath, ["--test", ...(args.includes("--runInBand") ? ["--test-concurrency=1"] : []), "test/deletion.test.ts"], { stdio: "inherit" });
process.exit(result.status ?? 1);
