# Security policy

## Reporting a vulnerability

**Do not open a public issue for security problems.** Instead, report privately via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on the affected repository (Security ▸ Report a vulnerability), or email the maintainer.

Please include the affected repo/version, a description, reproduction steps, and impact. We aim to
acknowledge within a few business days and will coordinate a fix and disclosure timeline with you.

## Scope

These components run at the edge and often hold credentials and talk to field devices. Take extra
care with anything touching the credentials vault, key providers, TLS material, or device write
paths.
