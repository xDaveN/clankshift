# Security policy

Please report vulnerabilities privately via
[GitHub private vulnerability reporting](https://github.com/xDaveN/clankshift/security/advisories/new),
not in public issues. Only the latest release is supported.

ClankShift never reads provider credentials itself; it runs the official `codex` and `claude` CLIs,
which use their own stored logins.
