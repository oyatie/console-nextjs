import { existsSync, statSync } from "node:fs";
import path from "node:path";

export function parseSingleBuckOutput(target, stdout) {
  const acceptedLabels = new Set([target, `root${target}`]);
  const outputLines = stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const match = line.match(/^(\S+)\s+(.+)$/);
      if (!match || !acceptedLabels.has(match[1])) return null;
      return match[2].trim();
    })
    .filter(Boolean);
  if (outputLines.length !== 1) {
    throw new Error(
      `Buck2 did not report exactly one output for ${target}; refusing to run an ambiguous artifact`,
    );
  }
  return outputLines[0];
}

// Buck's --show-output emits a path relative to the project root.  Treat that
// string as untrusted: only an existing executable physically addressed under
// this checkout's buck-out directory may become a development server.
export function resolveRepoBuckOutput(repoRoot, output) {
  if (path.isAbsolute(output)) {
    throw new Error("Buck2 reported an invalid Buck2 output; refusing to launch it");
  }
  const outputPath = path.resolve(repoRoot, output);
  const buckOut = path.resolve(repoRoot, "buck-out");
  const relative = path.relative(buckOut, outputPath);
  if (
    relative === "" ||
    relative === ".." ||
    relative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relative)
  ) {
    throw new Error("Buck2 reported an invalid Buck2 output; refusing to launch it");
  }
  if (!existsSync(outputPath)) {
    throw new Error("Buck2 output does not exist; refusing to launch it");
  }
  if (process.platform !== "win32" && (statSync(outputPath).mode & 0o111) === 0) {
    throw new Error("Buck2 output is not executable; refusing to launch it");
  }
  return outputPath;
}
