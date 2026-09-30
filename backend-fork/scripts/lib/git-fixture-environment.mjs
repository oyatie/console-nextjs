const FIXED_GIT_FIXTURE_ENVIRONMENT = Object.freeze({
  GIT_ASKPASS: "/bin/false",
  GIT_CONFIG_COUNT: "0",
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_SYSTEM: "/dev/null",
  GIT_NO_REPLACE_OBJECTS: "1",
  GIT_TERMINAL_PROMPT: "0",
  LC_ALL: "C",
});

/**
 * Return a child-process environment that cannot inherit repository discovery,
 * alternate object/index, config-injection, hook, signing, or transport controls.
 * Callers may add deliberate fixture-local Git values through `overrides`; those
 * are applied last. HOME and XDG variables are ordinary inputs and stay intact.
 */
export function gitFixtureEnvironment(sourceEnvironment = process.env, overrides = {}) {
  const inherited = Object.fromEntries(
    Object.entries(sourceEnvironment).filter(([name]) => !name.startsWith("GIT_")),
  );
  return { ...inherited, ...FIXED_GIT_FIXTURE_ENVIRONMENT, ...overrides };
}

/** Seal this test process so direct children and children of imported gates agree. */
export function installGitFixtureEnvironment(overrides = {}) {
  const environment = gitFixtureEnvironment(process.env, overrides);
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("GIT_")) Reflect.deleteProperty(process.env, name);
  }
  Object.assign(process.env, environment);
  return environment;
}
