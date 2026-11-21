# Project Agent Instructions

## Scope

This repository contains a Rust command-line utility that implements GPU MUX
diagnostics and switching on Windows and Linux for the characterized MSI laptops
listed in `HARDWARE_PROFILES` in `src/lib.rs`. Each entry is an exact model,
motherboard, and BIOS combination.

On Linux, UEFI access uses efivarfs and MSI ACPI access uses only the in-tree
`msi-wmi-platform` driver's root-only debugfs interface. Do not add a direct AML
call or a fallback transport without a new, deliberate safety review.

Treat all firmware writes and MSI ACPI calls as safety-critical. Prefer an
explicit failure over guessing a hardware contract or continuing from an
ambiguous state.

## Repository map

- `src/lib.rs` contains mode encoding, shared protocol validation, diagnostics,
  and platform-independent tests.
- `src/platform/mod.rs` defines the shared platform and ACPI traits and selects
  the native implementation. `src/platform/windows.rs` and
  `src/platform/linux.rs` contain all OS-specific system access.
- `src/bin/msi-mux-switch.rs` contains the CLI, reporting, safety gates,
  confirmation, backup metadata, apply sequence, and rollback handling.
- `build.rs` and `resources/` embed the Windows manifest that requires
  Administrator privileges.

Keep reusable protocol and system-access code in the library. Keep user
interaction and top-level transaction orchestration in the binary.

## Firmware contract and invariants

Do not change these details without new, deliberate characterization and tests:

- The UEFI variable is `MsiDCVarData`, GUID
  `{DD96BAAF-145E-4F56-B1CF-193256298E99}`.
- A writable value must be exactly 20 bytes and have attributes `0x00000007`.
- Byte 5 bits 0-1 are the selected target: `0` MSHybrid, `1` discrete, and `2`
  integrated.
- Byte 5 bits 2-3 report the current mode.
- Staging a target must preserve every other bit and byte.
- Byte 5 bit 4 advertises the characterized switch interface. Bit 5 advertises
  integrated support. A clear bit 6 advertises discrete support.
- `MSI_ACPI` must resolve to instance `ACPI\\PNP0C14\\0_0` in `ROOT\\WMI`.
- `Get_AP` must report success before its data is used. Its returned data byte
  1 is byte 2 of the package, and bit 1 is the apply-ready flag.
- Derive the trigger value from the latest `Get_AP` data byte by preserving its
  upper six bits and setting its low two bits to `01`.
- The apply trigger is `Set_Data(0xD1, trigger)`. After apply-ready asserts, the
  acknowledgement is `Set_Data(0xBE, 0x02)`.

Read the live state again immediately before a write. Verify the complete value
and its attributes after a write. Do not silently coerce unknown mode values,
variable layouts, ACPI results, or capabilities.

## Required safety behavior

Preserve these protections when changing the switching flow:

- Gate normal writes to the exact model and board of a `HARDWARE_PROFILES`
  entry, and gate the BIOS version separately against the BIOS versions of
  every entry with that model and board. On a machine that matches no entry,
  `--allow-unsupported-hardware` also covers the BIOS, because no validated
  version exists to compare against. Read-only diagnostics require an exact
  model, board, and BIOS match unless that flag is given. Bypasses must remain
  explicit and conspicuously named.
- Require an elevated Administrator process and online AC power.
- Require the mode-specific typed confirmation for human-readable interactive
  switching. JSON mode deliberately skips this prompt for scripted use.
- Refuse to start when apply-ready is already asserted.
- Create the rollback metadata before staging the firmware target.
- Restore the previous target when a post-write step fails, then verify the
  restoration. Report restoration failures without hiding the original error.
- Warn prominently when a trigger was sent before a later failure.
- Never automatically reboot or shut down the user's machine. A successful
  transition must instruct the user to save work and perform a full shutdown.
- Do not turn an override flag into a claim of hardware support.

Keep probes and tests that can mutate firmware opt-in and separate from the
normal unit-test suite. Never exercise real firmware writes merely to validate
a refactor.

## CLI and JSON compatibility

Treat scripted output as a public interface:

- With `--json`, mode switching emits one JSON object per line on stdout.
- Warnings, prompts, CLI errors, and runtime errors go to stderr.
- Do not print non-JSON text to stdout in JSON mode.
- Debug JSON is a single `StateSnapshot` document. Increment `schema_version`
  when making an incompatible schema change.
- `--json` deliberately skips typed confirmation. It must not weaken elevation,
  power, hardware, BIOS, firmware validation, or ACPI preflight checks.

Human-readable errors should say what failed and whether state may have
changed. Preserve stable event names unless a compatibility break is intended
and documented. Record user-visible changes under the `Unreleased` section in
`CHANGELOG.md`.

## Clean-room and repository hygiene

Static inspection may be used to determine an unclear contract, but inspected
binaries and material reproduced from their bytes must remain local and
untracked. Do not record the identity or provenance of an inspected binary in
source, documentation, tests, fixtures, commit messages, or logs. Persist only
the resulting independently stated contract and original test evidence.

Do not commit raw UEFI variable contents or machine-specific diagnostic
captures. Tests should use small synthetic values that express only the
contract under test.

## Required validation

Before completing any Rust change, both formatting and Clippy must pass with no
warnings:

```text
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
```

Run the test suite on Windows as well:

```text
cargo test --locked
```

The crate deliberately does not compile for non-Windows targets. From another
host, install a Windows standard library and give Clippy an explicit Windows
target:

```text
rustup target add x86_64-pc-windows-msvc
cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings
```

The build script skips Windows resource compilation only when Cargo is running
Clippy. Normal Windows builds must always compile and embed the required
Administrator manifest.

For protocol changes, add focused tests proving that unrelated bits and bytes
remain unchanged, invalid layouts are rejected, and all new decoded values are
handled explicitly. Review the human and JSON paths together.

## CI and releases

- `.github/workflows/ci.yml` runs formatting, strict Clippy, tests, and release
  builds for Windows AMD64 and Linux AMD64 on pull requests and relevant branch
  pushes.
- `.github/workflows/release.yml` builds and retains Windows AMD64 and Linux
  AMD64 ZIPs on pull requests and `master` pushes. A pushed `v*` tag publishes
  the archives and aggregate `SHA256SUMS` as a GitHub Release; the workflow
  does not create the tag.
- Release archives contain `msi-mux-switch.exe`, `README.md`, `CHANGELOG.md`,
  and `LICENSE`.
- Keep external GitHub Actions pinned to full commit hashes with a version
  comment. Do not replace pins with movable tags or branches.
