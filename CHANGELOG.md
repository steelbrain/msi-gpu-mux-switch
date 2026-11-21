# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-11-21

### Added

- Support for the MSI Stealth 16Studio A13VG with the MS-15F2 motherboard and
  BIOS E15F2IMS.10F. Switching has been tested on Linux; Windows switching on
  this model is untested.
- Support for BIOS E15M3IMS.10E on the MSI Vector 16 HX AI A2XWIG with the
  MS-15M3 motherboard. Switching between MSHybrid and discrete has been tested
  on Linux; integrated mode and Windows switching on this BIOS are untested.
- Supported systems are now listed as model, motherboard, and BIOS profiles.
  A model and motherboard can have more than one validated BIOS, and the BIOS
  error names each of them.

### Changed

- On a machine that matches no supported model and motherboard,
  `--allow-unsupported-hardware` now also skips the BIOS check.
  `--allow-unvalidated-bios` applies only to a recognized model and
  motherboard.
- `--debug` and the debug report's `expected_hardware` field now also require
  the BIOS to match. A supported laptop running a different BIOS needs
  `--allow-unsupported-hardware` to collect diagnostics.

## [0.2.0] - 2026-08-31

### Added

- Linux diagnostics and mode switching using DMI, efivarfs, PCI sysfs, and the
  kernel `msi-wmi-platform` debugfs interface.
- Linux AMD64 CI coverage and release archives with checksums.

### Changed

- Linux switching preserves the efivarfs inode flags around exact firmware
  writes and retains the same hardware, BIOS, power, confirmation, preflight,
  verification, rollback, and manual-shutdown protections as Windows.
- Platform-specific system access now implements shared platform and ACPI
  traits in separate Windows and Linux modules.
- Reworked the README with cross-platform installation, usage, prerequisites,
  safety, rollback, JSON, and development guidance.

## [0.1.0] - 2026-08-31

### Added

- Windows CLI for selecting MSHybrid, discrete, or integrated GPU MUX modes on
  the MSI Vector 16 HX AI A2XWIG with the MS-15M3 motherboard.
- Interactive mode selection and positional mode aliases.
- Structured JSON diagnostics and newline-delimited JSON switching events.
- Exact hardware and BIOS gates, Administrator and AC-power checks, firmware
  layout validation, write verification, rollback metadata, and automatic
  target restoration on apply failures.
- MSI ACPI apply handshake with readiness polling and acknowledgement.
- Embedded Windows manifest requesting Administrator privileges.
- Windows CI, release archives, SHA-256 checksums, and tagged GitHub Releases.

[Unreleased]: https://github.com/steelbrain/msi-gpu-mux-switch/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/steelbrain/msi-gpu-mux-switch/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/steelbrain/msi-gpu-mux-switch/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/steelbrain/msi-gpu-mux-switch/releases/tag/v0.1.0
