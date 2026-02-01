# Security Policy

## Threat model (alpha)

Oxidalloc is a general-purpose allocator for Linux. It is designed to reduce
allocator-specific risk, but it is not a security product and it does not
attempt to provide complete exploitation resistance.

### In scope

- Preventing allocator metadata corruption from silently going unnoticed.
- Reducing exploitation primitives from common heap corruption patterns.
- Hardening optional linked-list structures against pointer spoofing.
- Keeping allocator behavior deterministic and bounded under memory pressure.

### Out of scope

- Defending against arbitrary memory corruption in the host application.
- Supporting non-Linux platforms or non-standard kernels (may be supported in the future).
- Providing side-channel resistance or formal verification guarantees.

### Assumptions

- The host process is not already compromised.
- The kernel provides correct syscall behavior and basic ASLR.
- The application does not intentionally violate allocator contracts.

## Hardening features

Oxidalloc includes optional hardening features intended to raise the bar for
corruption bugs, but they are not audited.

- `hardened-malloc`: validates metadata and magic values to detect corruption.
- `hardened-linked-list`: XOR-masks pointers and strengthens list integrity.

These features can add overhead and are not a substitute for secure coding.

## Reporting vulnerabilities

Please report security issues **privately**.

- Open a minimal, non-public advisory (or email, if preferred).
- Include a proof-of-concept, environment details, and allocator config.
- I will acknowledge receipt and work with you on coordinated disclosure.

If you are unsure whether an issue is security-sensitive, report it privately
first.
