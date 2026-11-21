# MSI GPU MUX Switch

`msi-mux-switch` is a Windows and Linux command-line utility for inspecting and
changing the GPU MUX mode on the MSI laptops listed under
[Hardware support](#hardware-support). It implements the
switching flow without requiring MSI Center.

Available modes:

- `mshybrid` — makes both GPUs available to the operating system
- `discrete` — selects the dedicated GPU
- `integrated` — selects the integrated GPU

Aliases such as `hybrid`, `ms-hybrid`, `dgpu`, `igpu`, and `uma` are accepted.

> [!CAUTION]
> Switching writes an OEM UEFI variable and invokes OEM ACPI methods. A failed,
> interrupted, or incorrect firmware operation can require recovery and may
> prevent the machine from booting. Connect AC power, save your work, and keep
> any disk-encryption recovery key available off the laptop.

## Hardware support

The firmware contract is characterized for exactly these systems:

| Component | Required value |
| --- | --- |
| Laptop | MSI Vector 16 HX AI A2XWIG |
| Motherboard | MS-15M3 |
| BIOS | E15M3IMS.116 |
| Architecture | AMD64 |
| Operating system | Windows or Linux (untested) |

| Component | Required value |
| --- | --- |
| Laptop | MSI Vector 16 HX AI A2XWIG |
| Motherboard | MS-15M3 |
| BIOS | E15M3IMS.10E |
| Architecture | AMD64 |
| Operating system | Windows (untested) or Linux |

| Component | Required value |
| --- | --- |
| Laptop | MSI Stealth 16Studio A13VG |
| Motherboard | MS-15F2 |
| BIOS | E15F2IMS.10F |
| Architecture | AMD64 |
| Operating system | Windows (untested) or Linux |

Normal switching requires an exact model and motherboard match with one of
these systems. The BIOS is then checked separately against that system's
validated versions. Diagnostics with `--debug` require the model, motherboard,
and BIOS to all match. Development override flags exist, but they do not turn
an unknown machine or BIOS into a supported configuration.

## Installation

### Windows

Download `windows-amd64-msi-mux-switch.zip` and `SHA256SUMS` from the
[latest release](https://github.com/steelbrain/msi-gpu-mux-switch/releases).
Verify the archive, extract it, and run `msi-mux-switch.exe`. Its embedded
manifest requests Administrator privileges.

To build it yourself, install Rust 1.85 or newer and the MSVC build tools, then
run:

```powershell
cargo build --locked --release
```

The executable is written to `target\release\msi-mux-switch.exe`.

### Linux

Linux switching requires:

- Linux 6.10 or newer with the in-tree `msi-wmi-platform` driver
- EFI runtime services with efivarfs mounted at `/sys/firmware/efi/efivars`
- debugfs mounted at `/sys/kernel/debug`
- root privileges for switching and full ACPI diagnostics
- Rust 1.85 or newer when building from source

Download `linux-amd64-msi-mux-switch.zip` and `SHA256SUMS` from the
[latest release](https://github.com/steelbrain/msi-gpu-mux-switch/releases),
verify the archive, and extract it. The archive contains the
`msi-mux-switch` executable and project documentation.

Alternatively, build the release binary from source with:

```bash
cargo build --locked --release
```

The executable is written to `target/release/msi-mux-switch`.

The program checks that the expected MSI WMI device is attached to the
characterized ACPI parent, bound to `msi-wmi-platform`, and exposed through the
driver's root-owned mode-`0600` debugfs method files. It fails before staging a
firmware target if those checks do not pass.

## Usage

Run without a mode for the interactive selector:

```powershell
# Windows
.\msi-mux-switch.exe
```

```bash
# Linux
sudo ./msi-mux-switch
```

Or provide the target directly:

```powershell
.\msi-mux-switch.exe mshybrid
.\msi-mux-switch.exe discrete
.\msi-mux-switch.exe integrated
```

```bash
sudo ./msi-mux-switch mshybrid
sudo ./msi-mux-switch discrete
sudo ./msi-mux-switch integrated
```

Interactive switching requires the mode-specific typed confirmation. The
program never reboots, shuts down, or powers off the laptop automatically.

After a successful apply:

1. Save all work and close applications.
2. Perform a full shutdown.
3. Power the laptop on again.
4. Run `--debug` to verify the reported current mode.

## Read-only diagnostics

Diagnostics do not stage a target or invoke a write method:

```powershell
.\msi-mux-switch.exe --debug
.\msi-mux-switch.exe --debug --json
```

```bash
./msi-mux-switch --debug
./msi-mux-switch --debug --json
```

The report includes:

- detected manufacturer, model, motherboard, and BIOS
- Secure Boot state
- UEFI variable length, attributes, selected target, and current mode
- advertised MUX capabilities
- MSI ACPI `Get_AP` state when accessible
- detected display controllers
- relevant MSI Center registry values on Windows

On Linux, the root-only debugfs permissions mean an ordinary-user diagnostic
usually reports `Get_AP` as unavailable. Run the diagnostic with `sudo` only if
that additional read is needed.

## JSON output

Add `--json` to a switching command to emit one JSON event per line on standard
output:

```bash
sudo ./msi-mux-switch discrete --json
```

Warnings, prompts, CLI errors, and runtime errors go to standard error. Debug
mode emits one formatted `StateSnapshot` document with `schema_version` set to
`1`.

JSON switching deliberately skips typed confirmation for automation. It does
not skip hardware, BIOS, privilege, AC-power, firmware-layout, or ACPI
preflight checks.

## Safety and rollback

Before changing firmware, the utility:

1. Verifies the exact model, motherboard, and BIOS unless an explicit override
   was supplied.
2. Requires Administrator or root privileges and online AC power.
3. Validates the MUX capabilities and refuses an unknown mode encoding.
4. Refuses to continue if ACPI apply-ready is already asserted.
5. Reads the UEFI variable again immediately before the write.
6. Requires an exact 20-byte value with attributes `0x00000007`.
7. Records rollback metadata and a SHA-256 hash without copying the raw UEFI
   value.
8. Changes only byte 5 target bits 0–1.
9. Reads back and verifies the complete value and its attributes.
10. Performs the characterized trigger, readiness, and acknowledgement
    handshake.

If a post-write step fails, the previous target is restored and verified. If a
trigger was already sent, the error prominently warns that machine state may
have changed and recommends a full shutdown.

Rollback metadata is stored under:

- Windows: `%LOCALAPPDATA%\msi-gpu-mux\backups`
- Linux: `$XDG_STATE_HOME/msi-gpu-mux/backups`, falling back to
  `$HOME/.local/state/msi-gpu-mux/backups`

On Linux, the original efivarfs inode flags are restored after every attempted
write.

## Options

| Option | Meaning |
| --- | --- |
| `MODE` | Target mode, or omit it for the interactive selector |
| `--debug` | Collect read-only diagnostics and exit |
| `--json` | Emit structured output and skip typed confirmation |
| `--allow-unsupported-hardware` | Deliberately bypass the model/board gate; on an unrecognized machine, also skips the BIOS check |
| `--allow-unvalidated-bios` | Deliberately permit a BIOS that is not listed for a recognized model and motherboard |
| `--help` | Show complete command help |

The override flags are intended for deliberate characterization work. They do
not claim that the detected hardware supports the switching protocol.

## Development

The platform-independent protocol and transaction orchestration live in
`src/lib.rs` and `src/bin/msi-mux-switch.rs`. Native system access is isolated
behind shared traits in `src/platform/`, with separate Windows and Linux
implementations.

Required checks:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

From Linux, also validate the Windows code with:

```bash
rustup target add x86_64-pc-windows-msvc
cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings
```

Do not use real firmware writes merely to validate a refactor. See
[AGENTS.md](AGENTS.md) for the complete protocol invariants and safety rules.

Release history is recorded in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under the [MIT License](LICENSE).
