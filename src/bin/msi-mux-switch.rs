use std::fs;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use anyhow::{Result, bail};
use chrono::Local;
use clap::{Parser, error::ErrorKind};
use msi_gpu_mux::{
    EXPECTED_VARIABLE_ATTRIBUTES, EXPECTED_VARIABLE_LENGTH, FirmwareInfo, FirmwareVariable,
    MsiAcpi, ac_power_online, backup_directory, is_elevated, privilege_requirement, query_machine,
    read_msi_variable, recovery_key_warning, sha256_hex, shutdown_command, stage_target,
    trigger_value, validated_bios_versions, verification_command, wait_for_keypress,
    write_msi_variable,
};
use msi_gpu_mux::{Mode, StateSnapshot, collect_snapshot};
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// Inspect or change the GPU MUX mode on the characterized MSI MS-15M3.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Target mode. If omitted, an interactive menu is shown.
    #[arg(value_name = "MODE", conflicts_with = "debug")]
    mode: Option<Mode>,

    /// Collect read-only machine and MUX diagnostics, then exit.
    #[arg(long)]
    debug: bool,

    /// Emit JSON and skip typed confirmation. Progress is emitted as JSONL events.
    #[arg(long)]
    json: bool,

    /// Deliberately bypass the exact model/board gate. On an unrecognized
    /// machine, this also skips the BIOS check.
    #[arg(long)]
    allow_unsupported_hardware: bool,

    /// Deliberately permit a BIOS other than the characterized versions on a
    /// recognized model and board.
    #[arg(long)]
    allow_unvalidated_bios: bool,
}

#[derive(Debug, Serialize)]
struct BackupRecord {
    created_at: String,
    model: String,
    board: String,
    bios: String,
    requested_target: u8,
    requested_target_name: Mode,
    previous_target: u8,
    previous_target_name: Mode,
    previous_current: u8,
    byte5_before: String,
    byte5_staged: String,
    variable_length: usize,
    attributes: String,
    variable_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOutcome {
    Applied,
    Finished,
}

struct Reporter {
    json: bool,
}

impl Reporter {
    fn event(&self, event: &str, message: impl AsRef<str>, data: Value) -> Result<()> {
        let message = message.as_ref();
        if self.json {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "event": event,
                    "message": message,
                    "data": data,
                }))?
            );
        } else {
            println!("{message}");
        }
        Ok(())
    }

    fn warning(&self, event: &str, message: impl AsRef<str>, data: Value) {
        let message = message.as_ref();
        if self.json {
            eprintln!(
                "{}",
                json!({
                    "event": event,
                    "level": "warning",
                    "message": message,
                    "data": data,
                })
            );
        } else {
            eprintln!("warning: {message}");
        }
    }

    fn error(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.json {
            eprintln!(
                "{}",
                json!({
                    "event": "error",
                    "level": "error",
                    "message": message,
                })
            );
        } else {
            eprintln!("error: {message}");
        }
    }

    fn prompt(&self, event: &str, message: &str) -> Result<String> {
        if self.json {
            eprintln!(
                "{}",
                json!({
                    "event": event,
                    "level": "prompt",
                    "message": message,
                })
            );
            io::stderr().flush()?;
        } else {
            print!("{message}");
            io::stdout().flush()?;
        }

        let mut value = String::new();
        io::stdin().read_line(&mut value)?;
        Ok(value.trim().to_string())
    }
}

fn main() {
    let json_requested = std::env::args_os().any(|arg| arg == "--json");
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error) => {
            let informational = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            let exit_code = error.exit_code();
            if json_requested && !informational {
                eprintln!(
                    "{}",
                    json!({
                        "event": "cli_error",
                        "level": "error",
                        "message": error.to_string(),
                    })
                );
            } else {
                let _ = error.print();
            }
            if !informational && !json_requested {
                wait_for_keypress();
            }
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
            return;
        }
    };

    let reporter = Reporter { json: args.json };
    match run(args, &reporter) {
        Ok(outcome) => {
            if outcome == RunOutcome::Applied && !reporter.json {
                wait_for_keypress();
            }
        }
        Err(error) => {
            reporter.error(format!("{error:#}"));
            if !reporter.json {
                wait_for_keypress();
            }
            std::process::exit(1);
        }
    }
}

fn choose_mode(available: &[Mode], reporter: &Reporter) -> Result<Option<Mode>> {
    loop {
        let answer = reporter.prompt("mode_prompt", "Choose a mode number, or Q to cancel: ")?;
        if answer.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        if let Ok(index) = answer.parse::<usize>()
            && (1..=available.len()).contains(&index)
        {
            return Ok(Some(available[index - 1]));
        }
        reporter.warning("invalid_selection", "Invalid selection.", Value::Null);
    }
}

fn print_debug_text(snapshot: &StateSnapshot) {
    println!("MSI GPU MUX debug state");
    println!("Captured: {}", snapshot.captured_at);
    println!("Elevated: {}", snapshot.elevated);
    println!(
        "Machine: {} {} (board {} {}, BIOS {})",
        snapshot.machine.manufacturer,
        snapshot.machine.model,
        snapshot.machine.board,
        snapshot.machine.board_revision,
        snapshot.machine.bios
    );
    println!("Expected hardware: {}", snapshot.machine.expected_hardware);

    match (&snapshot.secure_boot.value, &snapshot.secure_boot.error) {
        (Some(enabled), _) => println!("Secure Boot: {enabled}"),
        (_, Some(error)) => println!("Secure Boot: unavailable ({error})"),
        _ => println!("Secure Boot: unavailable"),
    }

    println!("Registry:");
    for (name, value) in &snapshot.registry {
        match value {
            Some(value) => println!("  {name}: {value}"),
            None => println!("  {name}: unavailable"),
        }
    }

    match (&snapshot.firmware.value, &snapshot.firmware.error) {
        (Some(firmware), _) => {
            println!("Firmware:");
            println!(
                "  MsiDCVarData: length {}, attributes {}, byte 5 {}",
                firmware.length, firmware.attributes, firmware.byte5
            );
            println!(
                "  Selected target: {} ({})",
                firmware
                    .selected_target_mode
                    .map_or_else(|| "unknown".to_string(), |mode| mode.to_string()),
                firmware.selected_target_value
            );
            println!(
                "  Current mode: {} ({})",
                firmware
                    .current_mode
                    .map_or_else(|| "unknown".to_string(), |mode| mode.to_string()),
                firmware.current_value
            );
            println!(
                "  Capabilities: new-switch={}, discrete={}, integrated={}",
                firmware.new_switch_supported,
                firmware.discrete_supported,
                firmware.integrated_supported
            );
        }
        (_, Some(error)) => println!("Firmware: unavailable ({error})"),
        _ => println!("Firmware: unavailable"),
    }

    match (&snapshot.wmi_ap.value, &snapshot.wmi_ap.error) {
        (Some(ap), _) => {
            println!("MSI ACPI Get_AP:");
            println!("  Raw prefix: {}", ap.raw_prefix);
            println!("  Data byte 1: 0x{:02X}", ap.data_byte1);
            println!("  Apply-ready: {}", ap.apply_ready);
        }
        (_, Some(error)) => println!("MSI ACPI Get_AP: unavailable ({error})"),
        _ => println!("MSI ACPI Get_AP: unavailable"),
    }

    println!("Display devices:");
    for device in &snapshot.display_devices {
        println!(
            "  {}: present={:?}, status={}, problem={:?}",
            device.friendly_name,
            device.present,
            device.status.as_deref().unwrap_or("unknown"),
            device.problem_code
        );
        for hardware_id in &device.hardware_ids {
            println!("    {hardware_id}");
        }
    }
}

fn restore_previous_target(previous: Mode) -> Result<()> {
    let current = read_msi_variable()?;
    if current.bytes.len() != EXPECTED_VARIABLE_LENGTH {
        bail!("cannot restore target: MsiDCVarData length changed unexpectedly");
    }
    let restored = FirmwareVariable {
        bytes: stage_target(&current.bytes, previous)?,
        attributes: current.attributes,
    };
    write_msi_variable(&restored)?;
    let verified = read_msi_variable()?;
    if verified.bytes != restored.bytes || verified.attributes != restored.attributes {
        bail!("previous target restoration could not be verified");
    }
    Ok(())
}

fn run(args: Args, reporter: &Reporter) -> Result<RunOutcome> {
    if args.debug {
        let snapshot = collect_snapshot(args.allow_unsupported_hardware)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        } else {
            print_debug_text(&snapshot);
        }
        return Ok(RunOutcome::Finished);
    }

    let machine = query_machine()?;
    let validated_bios = validated_bios_versions(&machine);

    if validated_bios.is_empty() && !args.allow_unsupported_hardware {
        bail!(
            "unsupported hardware: model '{}', board '{}'; \
             use the explicit override only after reviewing the firmware layout",
            machine.model,
            machine.board
        );
    }

    if !validated_bios.is_empty()
        && !validated_bios.contains(&machine.bios.as_str())
        && !args.allow_unvalidated_bios
    {
        let characterized = validated_bios
            .iter()
            .map(|bios| format!("'{bios}'"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "BIOS '{}' is not characterized for this model and board \
             (characterized: {characterized}); \
             use --allow-unvalidated-bios only after reviewing firmware changes",
            machine.bios
        );
    }

    let initial_variable = read_msi_variable()?;
    let initial_info = FirmwareInfo::decode(&initial_variable)?;
    if !initial_info.new_switch_supported {
        bail!("firmware does not advertise the characterized three-mode switch");
    }
    let available = initial_info.available_modes();
    let current = initial_info
        .current_mode
        .context("firmware current-mode bits are not recognized")?;

    reporter.event(
        "current_mode",
        format!("Current mode: {current}"),
        json!({ "mode": current, "value": current.value() }),
    )?;
    let menu = available
        .iter()
        .enumerate()
        .map(|(index, mode)| format!("  {}. {mode}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    reporter.event(
        "available_modes",
        format!("Available modes:\n{menu}"),
        json!({ "modes": available }),
    )?;

    let target = match args.mode {
        Some(mode) => mode,
        None => match choose_mode(&available, reporter)? {
            Some(mode) => mode,
            None => {
                reporter.event("cancelled", "Cancelled. No state was changed.", Value::Null)?;
                return Ok(RunOutcome::Finished);
            }
        },
    };
    if !available.contains(&target) {
        bail!("mode '{target}' is not advertised as supported by this firmware");
    }

    let selected_target = initial_info
        .selected_target_mode
        .context("firmware selected/target bits are not recognized")?;
    if target == current && target == selected_target {
        reporter.event(
            "unchanged",
            format!("The machine is already in {target} mode. No state was changed."),
            json!({ "mode": target, "value": target.value() }),
        )?;
        return Ok(RunOutcome::Finished);
    }

    reporter.event(
        "selected_mode",
        format!("Selected mode: {target}"),
        json!({ "mode": target, "value": target.value() }),
    )?;
    reporter.event(
        "shutdown_required",
        "A full manual shutdown is required after a successful apply.\nThis program will not shut down or reboot the computer.",
        json!({ "automatic_shutdown": false }),
    )?;

    if !is_elevated()? {
        bail!("the GPU mode selector requires {}", privilege_requirement());
    }
    if !ac_power_online()? {
        bail!("AC power is required for a GPU mode transition");
    }

    reporter.warning("bitlocker_reminder", recovery_key_warning(), Value::Null);
    if args.json {
        reporter.event(
            "confirmation_skipped",
            "Typed confirmation skipped in JSON mode.",
            json!({ "json_mode": true }),
        )?;
    } else {
        let expected_confirmation = target.confirmation();
        let confirmation = reporter.prompt(
            "confirmation_prompt",
            &format!("Type '{expected_confirmation}' exactly to continue: "),
        )?;
        if confirmation != expected_confirmation {
            reporter.event(
                "confirmation_rejected",
                "Confirmation did not match. No state was changed.",
                Value::Null,
            )?;
            return Ok(RunOutcome::Finished);
        }
    }

    let acpi = MsiAcpi::connect()?;
    let ap_preflight = acpi.get_ap()?;
    if ap_preflight.apply_ready {
        bail!("apply-ready is already set before any write; refusing an ambiguous transition");
    }
    reporter.event(
        "preflight",
        format!(
            "Preflight Get_AP data byte 1: 0x{:02X}; apply-ready is clear.",
            ap_preflight.data_byte1
        ),
        json!({
            "data_byte1": ap_preflight.data_byte1,
            "apply_ready": false,
        }),
    )?;

    let before = read_msi_variable()?;
    if before.bytes.len() != EXPECTED_VARIABLE_LENGTH {
        bail!(
            "refusing write: expected a {}-byte MsiDCVarData value, found {}",
            EXPECTED_VARIABLE_LENGTH,
            before.bytes.len()
        );
    }
    if before.attributes != EXPECTED_VARIABLE_ATTRIBUTES {
        bail!(
            "refusing write: expected UEFI attributes 0x{EXPECTED_VARIABLE_ATTRIBUTES:08X}, found 0x{:08X}",
            before.attributes
        );
    }
    let before_info = FirmwareInfo::decode(&before)?;
    let previous_target = before_info
        .selected_target_mode
        .context("previous selected/target mode is not recognized")?;
    let staged = FirmwareVariable {
        bytes: stage_target(&before.bytes, target)?,
        attributes: before.attributes,
    };

    let backup_dir = backup_directory()?;
    fs::create_dir_all(&backup_dir)?;
    let backup_path = backup_dir.join(format!(
        "mux-target-{}.json",
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    let backup = BackupRecord {
        created_at: Local::now().to_rfc3339(),
        model: machine.model.clone(),
        board: machine.board.clone(),
        bios: machine.bios.clone(),
        requested_target: target.value(),
        requested_target_name: target,
        previous_target: previous_target.value(),
        previous_target_name: previous_target,
        previous_current: before_info.current_value,
        byte5_before: before_info.byte5,
        byte5_staged: format!("0x{:02X}", staged.bytes[5]),
        variable_length: before.bytes.len(),
        attributes: format!("0x{:08X}", before.attributes),
        variable_sha256: sha256_hex(&before.bytes),
    };
    fs::write(&backup_path, serde_json::to_vec_pretty(&backup)?)?;
    reporter.event(
        "rollback_record",
        format!("Rollback record: {}", backup_path.display()),
        json!({ "path": backup_path }),
    )?;

    let mut firmware_staged = false;
    let mut trigger_sent = false;

    let apply_result = (|| -> Result<()> {
        if let Err(error) = write_msi_variable(&staged) {
            firmware_staged = read_msi_variable()
                .map(|current| current.bytes != before.bytes)
                .unwrap_or(true);
            return Err(error);
        }
        firmware_staged = true;

        let after_stage = read_msi_variable()?;
        if after_stage.bytes != staged.bytes || after_stage.attributes != staged.attributes {
            bail!("UEFI target write verification failed");
        }
        reporter.event(
            "target_staged",
            format!(
                "UEFI target verified: byte 5 changed from 0x{:02X} to 0x{:02X}.",
                before.bytes[5], after_stage.bytes[5]
            ),
            json!({
                "byte5_before": format!("0x{:02X}", before.bytes[5]),
                "byte5_after": format!("0x{:02X}", after_stage.bytes[5]),
            }),
        )?;

        let ap_before = acpi.get_ap()?;
        if ap_before.apply_ready {
            bail!("apply-ready became set before the trigger; refusing an ambiguous transition");
        }
        let trigger = trigger_value(ap_before.data_byte1);
        reporter.event(
            "trigger",
            format!(
                "Get_AP data byte 1 before trigger: 0x{:02X}.\nSending characterized Set_Data trigger: address 0xD1, value 0x{trigger:02X}.",
                ap_before.data_byte1
            ),
            json!({
                "data_byte1_before": ap_before.data_byte1,
                "address": 0xd1,
                "value": trigger,
            }),
        )?;
        acpi.set_data(0xd1, trigger)?;
        trigger_sent = true;

        let deadline = Instant::now() + Duration::from_secs(5);
        let ready = loop {
            thread::sleep(Duration::from_millis(250));
            let state = acpi.get_ap()?;
            if state.apply_ready {
                break state;
            }
            if Instant::now() >= deadline {
                bail!("firmware did not assert apply-ready within five seconds");
            }
        };
        reporter.event(
            "apply_ready",
            format!(
                "Apply-ready asserted; Get_AP data byte 1 is 0x{:02X}.",
                ready.data_byte1
            ),
            json!({
                "data_byte1": ready.data_byte1,
                "apply_ready": true,
            }),
        )?;
        reporter.event(
            "acknowledgement",
            "Sending characterized Set_Data acknowledgement: address 0xBE, value 0x02.",
            json!({ "address": 0xbe, "value": 2 }),
        )?;
        acpi.set_data(0xbe, 0x02)?;
        Ok(())
    })();

    if let Err(error) = apply_result {
        if firmware_staged {
            match restore_previous_target(previous_target) {
                Ok(()) => reporter.warning(
                    "target_restored",
                    format!("restored previous target mode: {previous_target}"),
                    json!({ "mode": previous_target, "value": previous_target.value() }),
                ),
                Err(restore_error) => reporter.warning(
                    "rollback_failed",
                    format!("automatic target restoration also failed: {restore_error:#}"),
                    Value::Null,
                ),
            }
        }
        if trigger_sent {
            reporter.warning(
                "trigger_sent_before_failure",
                "the EC apply trigger was sent before failure; save work and perform a manual full shutdown",
                Value::Null,
            );
        }
        return Err(error);
    }

    let shutdown_command = shutdown_command();
    let verification_command = verification_command();
    reporter.event(
        "success",
        format!(
            "Mode '{target}' is staged and the firmware apply handshake succeeded.\nSave all work, close applications, then perform a full shutdown manually.\nRecommended manual command after saving work: {shutdown_command}\nAfter powering on, run {verification_command} to verify the new current mode."
        ),
        json!({
            "mode": target,
            "value": target.value(),
            "manual_shutdown_required": true,
            "verification_command": verification_command,
        }),
    )?;
    Ok(RunOutcome::Applied)
}
