// Test-process supervision only; this never certifies native enrollment.
export function superviseEnrollmentFault(child) {
  let forced = false;
  const deadline = setTimeout(() => { forced = true; child.kill("SIGTERM"); }, 25000);
  const force = setTimeout(() => { forced = true; child.kill("SIGKILL"); }, 35000);
  return {
    // Preserve the original spawn-relative clocks for the behavioral baseline.
    beginFault() {},
    get forced() { return forced; },
    clear() { clearTimeout(deadline); clearTimeout(force); },
  };
}
