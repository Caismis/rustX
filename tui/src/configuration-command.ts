/** Configuration commands are Rust-owned. Forward their arguments and streams. */
import { spawn } from "node:child_process";

export function isNativeConfigurationCommand(argument: string | undefined): boolean {
  return ["init", "config", "doctor", "workflow"].includes(argument ?? "");
}

export function configurationCommand(argv: readonly string[]): { binary: string; arguments: string[] } | undefined {
  if (argv[0] !== "--binary" || argv[1] === undefined) return undefined;
  if (!isNativeConfigurationCommand(argv[2]) && argv[2] !== "--help") return undefined;
  return { binary: argv[1], arguments: argv.slice(2) };
}

export async function forwardConfigurationCommand(command: { binary: string; arguments: string[] }): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(command.binary, command.arguments, { stdio: "inherit" });
    // Wait for Rust's owned settlement even when the launcher is interrupted.
    const interrupt = () => { child.kill("SIGINT"); };
    const terminate = () => { child.kill("SIGTERM"); };
    const message = (value: unknown) => {
      if (typeof value === "object" && value !== null && "stop" in value && value.stop === true) terminate();
    };
    process.on("message", message);
    process.on("disconnect", terminate);
    process.on("SIGINT", interrupt);
    process.on("SIGTERM", terminate);
    const finish = (code: number) => {
      process.off("message", message);
      process.off("disconnect", terminate);
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", terminate);
      resolve(code);
    };
    child.once("error", () => finish(1));
    child.once("close", (code) => finish(code ?? 1));
  });
}
