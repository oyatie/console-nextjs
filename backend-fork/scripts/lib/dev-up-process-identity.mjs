// A PID alone is not an authority to signal: it can be reused after a crash or
// restart. The launcher persists both the OS start token and command observed
// immediately after spawning, and permits shutdown only while both remain.
export function processIdentityMatches(expected, current) {
  return Boolean(
    expected?.startToken &&
      expected?.command &&
      current?.startToken === expected.startToken &&
      current.command === expected.command,
  );
}

export function shouldSignalManagedProcess(expected, current) {
  return processIdentityMatches(expected, current);
}

// Get-Process emits compact JSON so the Windows reader can make the same
// start-token-plus-executable comparison as the POSIX ps reader.
export function parseWindowsProcessIdentity(stdout) {
  try {
    const value = JSON.parse(stdout);
    return typeof value?.StartTime === "string" && typeof value?.Path === "string" &&
      value.StartTime && value.Path
      ? { startToken: value.StartTime, command: value.Path }
      : null;
  } catch {
    return null;
  }
}
