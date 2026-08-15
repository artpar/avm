# Security policy

## Supported versions

AVM is pre-1.0. Security fixes are applied to the latest release and `main`.
Older releases are not maintained.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting for this repository. If that is
unavailable, email `artpar@gmail.com` with the subject `AVM security report`.
Do not include credentials, production VM images, or sensitive run artifacts in
an initial report.

Please include the affected version or commit, impact, reproduction steps, and
any suggested mitigation. You should receive an acknowledgement within seven
days. Please allow time for a fix and coordinated disclosure before publishing
details.

## Security model

AVM treats candidate code and nested guests as untrusted. Supervisor state,
credentials, evaluator data, and immutable evidence must remain outside the
candidate workspace. A report that crosses one of those boundaries is
security-sensitive even when it does not provide host code execution.
