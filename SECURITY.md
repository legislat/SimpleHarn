---
title: Security Policy
owner: operator
status: active
audience: [user, security reporter]
source_of_truth_for: [vulnerability reporting, supported versions, supported security posture]
last_reviewed: 2026-11-08
review_trigger: release, security-change
---

# Security Policy

## Supported versions

The latest release on the `main` branch is supported. Older releases do not
receive fixes.

## Report a vulnerability

Use GitHub Security Advisories: open the repository Security tab and choose
"Report a vulnerability". Do not open a public issue for a security problem.

Include the version, the configuration, and the smallest steps that reproduce
the problem. Do not include live keys, private model files, or personal data.

## Scope and supported posture

SimpleHarn is a local, single-user harness. It binds loopback by default and
is not a multi-user service. The table states each bind and token state, what
the product does, and whether it is inside the supported security posture
for vulnerability reports.

| Bind and token state | Product behavior | In supported posture |
|---|---|---|
| Loopback bind, no control token | Default posture. The API is unauthenticated by default. | Yes |
| Any bind with a control token | Every route except the page assets requires the token. | Yes |
| Non-loopback bind, no control token | Boot refuses this state since 2026-11-08. It proceeds only on the explicit opt-in `allow_unauthenticated_network_bind: true`, and then shows a persistent warning banner on the control page on every read. The opt-in replaces the operator ruling of 2026-11-06 that kept the open bind allowed without one. | No |

The third row stays outside the supported posture even with the opt-in: the
opt-in is a deliberate boot gate, not an endorsement. Without it the harness
never binds the open posture at all; with it the harness warns on every
read. A report about the exposure itself is therefore out of scope. A report
about a broken warning, a harness that boots the open posture without the
opt-in, or a bug reachable from that posture is still welcome. The bind and
token keys live in `docs/reference/CONFIG.md`, and the warning behavior lives
in `README.md`. The harness forces every engine it starts onto a loopback
listener, whatever `internal_host` says. A configuration that exposes a
listener to a network without a control token stays outside the supported
posture; see `docs/reference/CONFIG.md` and `docs/reference/OPERATIONS.md`.
