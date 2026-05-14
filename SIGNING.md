# Signing Kefilex Desk

The release binaries on [app.kefilex.com/get-kefilex-desk](https://app.kefilex.com/get-kefilex-desk) are signed by [SignPath.io](https://signpath.io) under their free open-source program. This document explains how that's set up and what to do if it needs renewing.

## Why signed

Windows shows a SmartScreen warning ("publisher unknown — running this file could harm your computer") on every unsigned binary downloaded via a browser. Reception staff would reasonably refuse to install software that looks like that. SignPath's EV certificate makes the binary recognised as legitimate from the first install, no reputation-building required.

## How to apply (one-time, ~10 minutes)

1. Open https://about.signpath.io/product/open-source
2. Click **Apply for the Open-Source Plan**
3. Form fields:
   - **Project name:** Kefilex Desk
   - **Repository URL:** https://github.com/Kefilab/kefilex-desk
   - **License:** MIT
   - **Project description:** *Live incoming-call capture companion for the Kefilex law-firm SaaS. Watches OS notifications and reports calls to our backend.*
   - **Contact email:** info@kefilab.com
4. Submit and wait 3-10 days for approval

SignPath verifies the GitHub repo is genuinely open source (public, MIT license, real commit history) before approving. The repo is ready for this — `LICENSE`, `README.md`, real commits all in place.

## What SignPath sends you back

On approval you'll receive an email containing:

- A **SignPath organisation ID** (UUID)
- A **SignPath project ID** (UUID)
- A **signing policy ID** (UUID)
- An **API token** for CI integration

These four values go into the GitHub Actions secrets on the `Kefilab/kefilex-desk` repo:

```
SIGNPATH_ORGANIZATION_ID
SIGNPATH_PROJECT_SLUG
SIGNPATH_SIGNING_POLICY_SLUG
SIGNPATH_API_TOKEN
```

Once those are set, the existing `.github/workflows/release.yml` (lands in Phase 31d) will pick them up automatically. No code changes needed.

## What happens to unsigned binaries in the meantime

During Phase 31b development we build unsigned binaries locally and install them on Bal's Windows laptop for testing. Windows SmartScreen will say *"Windows protected your PC"* — click **More info → Run anyway** to install. That's expected for unsigned dev builds and only needs doing once per binary version.

For non-developer testers (JL reception staff), we wait until SignPath approval + the first signed release before handing them anything.

## Fallback if SignPath doesn't approve

If for any reason SignPath rejects the application (unlikely but possible), the fallback is **Microsoft Azure Trusted Signing**: ~£10/month, no GitHub-org-public requirement, cloud-managed signing keys, integrates with GitHub Actions via the `az trusted-signing` CLI. Setup time is ~30 minutes.

We pick Azure as the fallback rather than a traditional EV cert because it's the cheapest credible option that doesn't require an HSM hardware token.

## Why not self-signed?

Self-signed certificates produce the same SmartScreen warning as unsigned binaries. They're useful only for *the developer* on the machine that generated the cert — and even then SmartScreen still warns. No upside for distribution.

## Renewal cadence

- **SignPath open-source plan:** free indefinitely as long as the GitHub repo remains public and MIT-licensed. No renewal needed.
- **Azure Trusted Signing (if used as fallback):** monthly billing, no expiry to worry about as long as the subscription is active.
- **Traditional EV cert (if ever used):** 1-3 year cycles, requires renewal + re-issue of signing keys.

## Verifying a release was actually signed

After download, right-click the `.msi` → Properties → Digital Signatures tab. Should show:

- **Signer:** SignPath GmbH (or our org name once they configure it)
- **Timestamp:** an RFC 3161 timestamp from a trusted authority
- **Status:** "This digital signature is OK"

If you're paranoid, `signtool verify /pa /v KefilexDesk.msi` from a Visual Studio Developer command prompt does the same check programmatically.
