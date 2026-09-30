#!/usr/bin/env node
// Fail-slow sweep collector: print the failing step ids from a job's `steps`
// context and exit 1 if any step failed. A step that was SKIPPED (its `if`
// evaluated false, e.g. a dependent step whose dependency failed) is not a
// failure — only `outcome === "failure"` is. This re-asserts the job-level red
// that feeds `Required / CI` and names the root failure(s) so a lane sees the
// whole sweep in one look.
//
// The `steps` context is passed via CI_STEPS (`${{ toJSON(steps) }}` in the
// workflow step's env), because `steps.*.outcome` cannot be iterated from bash.

const steps = JSON.parse(process.env.CI_STEPS || "{}");
const failed = Object.entries(steps)
  .filter(([, step]) => step && step.outcome === "failure")
  .map(([id]) => id)
  .sort();

if (failed.length > 0) {
  console.error(`failed steps: ${failed.join(", ")}`);
  process.exit(1);
}
console.log("all steps passed");
