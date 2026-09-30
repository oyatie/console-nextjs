import { spawn } from "node:child_process";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const backendDir = resolve(root, "backend");
const targetDir = process.env.CARGO_TARGET_DIR ?? resolve(root, ".tmp/cargo-target-g012");

await runCommand("cargo", ["clippy", "-p", "console-payroll-domain", "--all-targets", "--", "-D", "warnings"], {
  cwd: backendDir,
  env: {
    ...process.env,
    CARGO_INCREMENTAL: process.env.CARGO_INCREMENTAL ?? "0",
    CARGO_TARGET_DIR: targetDir,
  },
  label: "cargo clippy -p console-payroll-domain --all-targets -- -D warnings",
});

await runCommand("cargo", ["test", "-p", "console-payroll-domain"], {
  cwd: backendDir,
  env: {
    ...process.env,
    CARGO_INCREMENTAL: process.env.CARGO_INCREMENTAL ?? "0",
    CARGO_TARGET_DIR: targetDir,
  },
  label: "cargo test -p console-payroll-domain",
});

async function runCommand(command, args, options) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const capture = (chunk) => {
    const text = chunk.toString();
    output += text;
    process.stderr.write(text);
  };
  child.stdout.on("data", capture);
  child.stderr.on("data", capture);
  const code = await new Promise((resolveCode, reject) => {
    child.on("error", reject);
    child.on("close", resolveCode);
  });
  if (code !== 0) {
    throw new Error(`${options.label} exited with ${code}\n${output}`);
  }
}
