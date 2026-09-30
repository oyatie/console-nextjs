import { afterEach, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

it("leaves legacy browser data untouched while keeping prototype changes in memory", async () => {
  const localStorage = {
    getItem: vi.fn().mockReturnValue(JSON.stringify({ employees: [] })),
    setItem: vi.fn(),
    removeItem: vi.fn(),
  };
  vi.stubGlobal("window", { localStorage });
  vi.resetModules();

  const { useAppStore } = await import("../src/lib/store");
  expect(useAppStore.getState().employees.length).toBeGreaterThan(0);
  expect(localStorage.getItem).not.toHaveBeenCalled();
  expect(localStorage.removeItem).not.toHaveBeenCalled();

  useAppStore.setState({ employees: [] });
  expect(useAppStore.getState().employees).toEqual([]);
  expect(localStorage.setItem).not.toHaveBeenCalled();
  expect(localStorage.removeItem).not.toHaveBeenCalled();
});
