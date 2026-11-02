# Security Policy

## Supported versions

Only the latest minor release line receives security fixes. At the time of this
writing that is **0.8.x**.

## Reporting a vulnerability

**Do not open a public issue for a security problem.** A public issue discloses
the vulnerability before a fix exists.

Use GitHub's private vulnerability reporting for this repository:
**Security tab → "Report a vulnerability"**. Reports reach the maintainers
privately, and you should receive a response within **7 days** (usually sooner).

If that channel is unavailable, contact the maintainer at
**cloudwalkerfree@gmail.com** with the subject line `MDIO security`.

Please include:

- the affected version (`mdio --version`) and how it was installed;
- platform: Linux x86_64, macOS arm64, or Windows via WSL2;
- reproduction steps or a minimal proof of concept;
- the impact you believe the issue has.

## Scope

This policy covers the code in this repository and the artifacts published from
it (the PyPI wheel and sdist). Vulnerabilities in third-party dependencies should
be reported to their own maintainers; dependency issues are tracked here through
GitHub's dependency alerts.
