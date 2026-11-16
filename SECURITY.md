# Security policy

Hoarder reads everything you copy and keeps History on disk, so a flaw that leaks Clips, Concealed copies or the History key matters. Thank you for reporting one privately.

## Supported versions

Only the latest release gets fixes. Upgrade before reporting.

## Reporting a vulnerability

Email **ping@cptdawn.dev** with "Hoarder security" in the subject. Include the Hoarder version (`hoarder --version`), your compositor, and the steps that show the problem. Never send real clipboard contents or History.

Please do not open a public issue for a vulnerability. A fix ships as a patch release with credit in the changelog unless you ask otherwise.

## Scope

How Hoarder treats sensitive copies is described in [docs/privacy.md](docs/privacy.md) and [ADR-0006](docs/adr/0006-three-sensitivity-tiers.md). A copy that escapes those rules (a Secret stored, a Concealed Clip written to disk, History readable by another user) is in scope.
