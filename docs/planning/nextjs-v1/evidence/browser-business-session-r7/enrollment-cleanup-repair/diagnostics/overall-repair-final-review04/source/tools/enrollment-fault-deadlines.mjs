// Test-process supervision only; this never certifies native enrollment.
export function superviseEnrollmentFault(child) {
  let forced = false;
  let faultStarted = false;
  let disposed = false;
  const terminate = () => { forced = true; child.kill("SIGTERM"); };
  const kill = () => { forced = true; child.kill("SIGKILL"); };
  // One absolute prerequisite budget, bounded by the runner's 140s watchdog.
  // It covers locked installation and real Next/Chromium startup, not cleanup.
  let deadline = setTimeout(terminate, 140000);
  let force = setTimeout(kill, 150000);
  return {
    beginFault() {
      if (forced || faultStarted || disposed) throw new Error("Fault deadline cannot be restarted");
      faultStarted = true;
      clearTimeout(deadline);
      clearTimeout(force);
      // Keep the original cleanup limits, starting at the actual fault input.
      // Both the cleanup acknowledgement AND natural exit must fit this clock.
      deadline = setTimeout(terminate, 25000);
      force = setTimeout(kill, 35000);
    },
    get forced() { return forced; },
    clear() { disposed = true; clearTimeout(deadline); clearTimeout(force); },
  };
}
