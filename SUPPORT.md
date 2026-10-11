# Support

## Where to ask

| Question type | Where |
| --- | --- |
| Bug report (compiler, runtime, standard library) | [Bug Report issue](.github/ISSUE_TEMPLATE/bug_report.md) |
| New language feature | [Feature Request issue](.github/ISSUE_TEMPLATE/feature_request.md) |
| Language change that alters semantics or syntax | [Language Proposal issue](.github/ISSUE_TEMPLATE/language_proposal.md) |
| Security vulnerability | [SECURITY.md](SECURITY.md) — private report, never a public issue |
| How to contribute | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Working on the compiler itself | A GitHub issue or pull request after reading `CONTRIBUTING.md` and `docs/self-hosting-plan.md` |

## Before you file

1. Update to the newest commit on the branch you build from and confirm the problem still happens.
2. Run `ryn --version` and include the output.
3. Reduce the input: one file, the smallest program that shows the behavior.
4. Record the exact command, the observed output (stdout, stderr, exit code) and the expected output.
5. Search existing issues, open and closed, for the same report.

A good report for a wrong-exit-code, wrong-output, or crash bug includes the command, the program,
the full output, and the target (Linux x86-64 or the experimental Windows x86-64). A good report for a diagnostic problem quotes the diagnostic code (for example `R0234`)
and the program line.

## Response expectations

Ryn is maintained by a small team. There is no paid support and no guaranteed response time. Reports
are triaged in the order received; a fix depends on severity and available time. A pull request that
includes a regression test is the fastest route to a fix.

## What not to use issues for

- Questions about programs you are writing yourself — those are ordinary bug reports only when the
  compiler is wrong, not when the program is.
- Support requests for third-party packages; contact their maintainers.
- Anything that belongs in the security process above.
