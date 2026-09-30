import { spawnSync } from "node:child_process";

export function hasJava() {
  return spawnSync("java", ["-version"], { stdio: "ignore" }).status === 0;
}

export function hasRunningDocker() {
  // `docker info` exits non-zero (and prints to stderr) when the CLI is present
  // but no daemon is reachable. spawnSync.status is null when `docker` itself is
  // not installed (ENOENT) — treat both as "no usable Docker". The timeout keeps
  // a wedged daemon socket from stalling the preflight indefinitely.
  return spawnSync("docker", ["info"], { stdio: "ignore", timeout: 10_000 }).status === 0;
}
