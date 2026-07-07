# Local Server before static report

We will build the HTML View as a localhost-only server before a static report. The product needs interactive inspection and conflict resolution for Library state, Exposures, update impact, and doctor findings, so a static report would quickly turn into a second-class surface. The Local Server must still share the same state and filesystem operation layer as the CLI, so it does not create a second source of truth.
