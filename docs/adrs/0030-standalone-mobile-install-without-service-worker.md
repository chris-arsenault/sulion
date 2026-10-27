# ADR 0030: Home-Screen Installation Without a Service Worker or Offline Cache

## Status

Accepted, 2026-10-16.

## Context

The phone is a first-class Sulion client for reviewing the timeline and
sending prompts, and the mobile layout already handled viewport sizing and
safe areas. Opening Sulion in a browser tab on a phone kept the browser
toolbar on screen and competed with the timeline for height. The user
asked whether Sulion could easily become a PWA.

Everything the mobile client shows comes from the server: session lists,
timeline rows, prompt submission, and terminal state. There is no local
work to do without a connection. `docs/backlog.md` had listed "Offline PWA
/ local sync" under features explicitly not recommended since the first
backlog pass (eb66472, 2026-04-18): the product is LAN-tethered by
definition, and sync invites divergence bugs with no user gain.

Installability in current browsers needs a web manifest, icons, and a
browser-trusted HTTPS origin. It does not need a service worker.

## Decision

Sulion ships the installation metadata and nothing more: a
`manifest.webmanifest` with `standalone` display, app name, and launch URL;
PNG icons generated from `frontend/public/favicon.svg` by `pnpm icons`
(including an Apple touch icon); theme metadata in `index.html`; and
`manifest-src 'self'` in the nginx security headers. The browser-security
e2e spec checks that the manifest and icons load under the production
headers.

The installed app is **online-first**. There is no service worker, no
offline cache, and no local sync. Launching from the home screen opens the
same application against the same server, without browser controls. The
user guide states that the installed app needs a connection and that
ordinary browser access, including existing LAN HTTP URLs, is unchanged.

## Alternatives considered

- **A full PWA with a service worker and offline cache.** Declined in the
  assessment that led to c9fbdbb (transcript 01a1466c…, turn 2117793):
  "keeping it online-first. Sessions and timeline updates need the server;
  a service worker and offline caching aren't required for installation."
  `docs/backlog.md` carries the standing product position that offline
  mode and local sync are not wanted. The CHANGELOG entry for the feature
  records that installation "adds no service worker or offline cache" so
  the boundary is explicit rather than an omission.
- **Leaving mobile as a browser tab.** The state before c9fbdbb. The cost
  of the change was small (manifest, icons, two header lines, one spec)
  and the toolbar-free window gives the timeline the height it lacked, so
  there was no reason to keep the tab-only experience.

## Consequences

- Installation requires an HTTPS origin the phone trusts. Plain LAN HTTP
  URLs keep working in a browser tab but do not meet install criteria.
- A service worker would have introduced a second cache of the frontend
  bundle and a deployment story for invalidating it. Without one, a
  release is live on the next load, same as the browser tab.
- Sign-in state may not carry over from the browser into the installed
  app; the user guide says to expect a second sign-in.
- Physical-device behaviour (keyboard, safe areas in standalone mode) was
  checked in Chromium under Playwright; installation on a real phone was
  not verified as part of the commit.
- Any future offline feature is a reopening of this decision and of the
  backlog's "not recommended" entry, not an incremental addition.

## References

- [`../user-guide.md`](../user-guide.md) — Launch from the home screen
- [`../backlog.md`](../backlog.md) — Explicitly NOT recommended
- [`../development.md`](../development.md) — E2E (icon generation)
- [`../../CHANGELOG.md`](../../CHANGELOG.md) — Unreleased, Mobile
  installation
- eb66472 — feature backlog naming offline PWA as not recommended
- c9fbdbb — standalone mobile app installation
