import "@testing-library/jest-dom";

// The test environment prefers reduced motion: jsdom's built-in matchMedia
// answers `matches: false` to every query, which reads as "motion allowed"
// and lets celebration/ceremony timers race async assertions. Declaring
// reduced-motion here makes every animated surface take its documented
// instant path, so tests assert the contract, not the choreography.
Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string): MediaQueryList =>
    ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList,
});
