/** Configuration commands are Rust-owned. Forward their arguments and streams. */
import { spawn } from "node:child_process";

export function configurationCommand(argv: readonly string[]): { binary: string; arguments: string[] } | undefined {
  if (argv[0] !== "--binary" || argv[1] === undefined) return undefined;
  if (!["init", "config", "doctor", "workflow", "--help"].includes(argv[2] ?? "")) return undefined;
  return { binary: argv[1], arguments: argv.slice(2) };
}

export async function forwardConfigurationCommand(command: { binary: string; arguments: string[] }): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(command.binary, command.arguments, { stdio: "inherit" });
    // Wait for Rust's owned settlement even when the launcher is interrupted.
    const interrupt = () => { child.kill("SIGINT"); };
    const terminate = () => { child.kill("SIGTERM"); };
    process.on("SIGINT", interrupt);
    process.on("SIGTERM", terminate);
    const finish = (code: number) => {
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", terminate);
      resolve(code);
    };
    child.once("error", () => finish(1));
    child.once("exit", (code) => finish(code ?? 1));
  });
}
