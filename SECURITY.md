# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for a security bug. Report it privately so it can be fixed
before it is disclosed:

- Use GitHub's private vulnerability reporting for this repository
  (`Security` → `Report a vulnerability`), or
- contact the repository owner privately through the contact information published on their GitHub
  profile ([github.com/v3nn7](https://github.com/v3nn7)).

This project has no dedicated security mailbox. Please do not send security reports to the public
issue tracker, discussions, or pull requests.

## What to include

A useful report contains:

- the affected component (`src/` compiler, Ryn standard library, or the native `rync` compiler under
  `selfhost/`) and the version from `ryn --version`;
- the input, command, or program that triggers the problem, and a minimal reproduction when possible;
- the observed behavior versus what you expected;
- your assessment of impact (for example: memory corruption, escaping Ryn Guard, incorrect code
  generation, or a build that runs untrusted code without consent).

## What to expect

- Acknowledgement of your report, normally within a few days.
- An assessment of severity and affected versions, and a target fix or mitigation.
- Credit in the security advisory and the changelog when the fix ships, unless you prefer to stay
  anonymous.

## Scope

The Ryn compiler, the runtime it links into generated programs, the standard library under
`stdlib/`, and the self-hosted compiler sources under `selfhost/` are in scope. Reports about programs
written *in* Ryn by third parties are out of scope; those are ordinary bug reports.

Compiling and running a Ryn program executes native machine code. Trusting the compiler with source
files is trust by design; memory-safety violations *inside* the compiler or the generated runtime,
and any way to make the compiler emit code that violates Ryn's documented safety rules, are what this
policy covers.

## Handling of disclosed issues

Fixed vulnerabilities are published as GitHub security advisories with a patch release or a guidance
note, and are recorded in [CHANGELOG.md](CHANGELOG.md).
