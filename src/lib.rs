use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod platform;

pub use platform::{
    MsiAcpi, ac_power_online, backup_directory, is_elevated, privilege_requirement,
    query_display_devices, query_machine, query_registry_state, read_msi_variable,
    recovery_key_warning, secure_boot_enabled, shutdown_command, verification_command,
    wait_for_keypress, write_msi_variable,
};

// Shared firmware-variable details. This implementation assumes these are the
// same for every hardware profile below.
pub const MSI_VARIABLE_NAME: &str = "MsiDCVarData";
pub const MSI_VARIABLE_GUID: &str = "{DD96BAAF-145E-4F56-B1CF-193256298E99}";
pub const EXPECTED_VARIABLE_LENGTH: usize = 20;
pub const EXPECTED_VARIABLE_ATTRIBUTES: u32 = 0x0000_0007;

/// A hardware combination this program recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareProfile {
    pub model: &'static str,
    pub board: &'static str,
    pub bios: &'static str,
}

/// Add one entry for each supported model/board/BIOS combination.
pub const HARDWARE_PROFILES: &[HardwareProfile] = &[
    HardwareProfile {
        model: "Vector 16 HX AI A2XWIG",
        board: "MS-15M3",
        bios: "E15M3IMS.116",
    },
    HardwareProfile {
        model: "Vector 16 HX AI A2XWIG",
        board: "MS-15M3",
        bios: "E15M3IMS.10E",
    },
    HardwareProfile {
        model: "Stealth 16Studio A13VG",
        board: "MS-15F2",
        bios: "E15F2IMS.10F",
    }, // Example:
       // HardwareProfile {
       //     model: "Another model",
       //     board: "Another board",
       //     bios: "Another BIOS",
       // },
];

pub(crate) const SECURE_BOOT_VARIABLE_NAME: &str = "SecureBoot";
pub(crate) const EFI_GLOBAL_VARIABLE_GUID: &str = "{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    MsHybrid,
    Discrete,
    Integrated,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::MsHybrid, Self::Discrete, Self::Integrated];

    pub const fn value(self) -> u8 {
        match self {
            Self::MsHybrid => 0,
            Self::Discrete => 1,
            Self::Integrated => 2,
        }
    }

    pub const fn from_value(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::MsHybrid),
            1 => Some(Self::Discrete),
            2 => Some(Self::Integrated),
            _ => None,
        }
    }

    pub fn confirmation(self) -> &'static str {
        match self {
            Self::MsHybrid => "APPLY MSHYBRID",
            Self::Discrete => "APPLY DISCRETE",
            Self::Integrated => "APPLY INTEGRATED",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MsHybrid => write!(f, "MSHybrid"),
            Self::Discrete => write!(f, "Discrete"),
            Self::Integrated => write!(f, "Integrated"),
        }
    }
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "mshybrid" | "ms-hybrid" | "hybrid" => Ok(Self::MsHybrid),
            "discrete" | "dgpu" => Ok(Self::Discrete),
            "integrated" | "igpu" | "uma" => Ok(Self::Integrated),
            _ => bail!("unknown GPU mode '{value}'"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FirmwareVariable {
    pub bytes: Vec<u8>,
    pub attributes: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FirmwareInfo {
    pub length: usize,
    pub attributes: String,
    pub byte5: String,
    pub selected_target_value: u8,
    pub selected_target_mode: Option<Mode>,
    pub current_value: u8,
    pub current_mode: Option<Mode>,
    pub new_switch_supported: bool,
    pub integrated_supported: bool,
    pub discrete_supported: bool,
    pub bit7: bool,
}

impl FirmwareInfo {
    pub fn decode(variable: &FirmwareVariable) -> Result<Self> {
        let byte5 = *variable
            .bytes
            .get(5)
            .context("MsiDCVarData is shorter than six bytes")?;

        let target = byte5 & 0x03;
        let current = (byte5 & 0x0c) >> 2;

        Ok(Self {
            length: variable.bytes.len(),
            attributes: format!("0x{:08X}", variable.attributes),
            byte5: format!("0x{byte5:02X}"),
            selected_target_value: target,
            selected_target_mode: Mode::from_value(target),
            current_value: current,
            current_mode: Mode::from_value(current),
            new_switch_supported: byte5 & 0x10 != 0,
            integrated_supported: byte5 & 0x20 != 0,
            discrete_supported: byte5 & 0x40 == 0,
            bit7: byte5 & 0x80 != 0,
        })
    }

    pub fn available_modes(&self) -> Vec<Mode> {
        if !self.new_switch_supported {
            return Vec::new();
        }

        let mut modes = vec![Mode::MsHybrid];

        if self.discrete_supported {
            modes.push(Mode::Discrete);
        }

        if self.integrated_supported {
            modes.push(Mode::Integrated);
        }

        modes
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineInfo {
    pub manufacturer: String,
    pub model: String,
    pub board: String,
    pub board_revision: String,
    pub bios: String,
    pub expected_hardware: bool,
}

/// Returns the first profile whose model, board, and BIOS all match.
pub fn matching_profile(machine: &MachineInfo) -> Option<&'static HardwareProfile> {
    HARDWARE_PROFILES.iter().find(|profile| {
        machine.model == profile.model
            && machine.board == profile.board
            && machine.bios == profile.bios
    })
}

/// Returns the validated BIOS versions of every profile with the machine's
/// model and board, in profile order. An empty list means the model and board
/// are not recognized.
pub fn validated_bios_versions(machine: &MachineInfo) -> Vec<&'static str> {
    HARDWARE_PROFILES
        .iter()
        .filter(|profile| machine.model == profile.model && machine.board == profile.board)
        .map(|profile| profile.bios)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplayDevice {
    pub friendly_name: String,
    pub status: Option<String>,
    pub problem_code: Option<u32>,
    pub present: Option<bool>,
    pub hardware_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApState {
    pub raw_prefix: String,
    pub flag: u8,
    pub data_byte1: u8,
    pub apply_ready: bool,
}

pub(crate) fn decode_ap_state(raw: &[u8]) -> Result<ApState> {
    if raw.len() < 3 || raw[0] == 0 {
        bail!("MSI ACPI Get_AP returned an unsuccessful or short package");
    }

    let prefix_len = raw.len().min(8);
    let raw_prefix = raw[..prefix_len]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(ApState {
        raw_prefix,
        flag: raw[0],
        data_byte1: raw[2],
        apply_ready: raw[2] & 0x02 != 0,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailableSection<T> {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> AvailableSection<T> {
    fn from_result(result: Result<T>) -> Self {
        match result {
            Ok(value) => Self {
                available: true,
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                available: false,
                value: None,
                error: Some(format!("{error:#}")),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub elevated: bool,
    pub machine: MachineInfo,
    pub secure_boot: AvailableSection<bool>,
    pub registry: BTreeMap<String, Option<u32>>,
    pub firmware: AvailableSection<FirmwareInfo>,
    pub wmi_ap: AvailableSection<ApState>,
    pub display_devices: Vec<DisplayDevice>,
}

pub fn stage_target(bytes: &[u8], target: Mode) -> Result<Vec<u8>> {
    if bytes.len() <= 5 {
        bail!("MsiDCVarData is shorter than six bytes");
    }

    let mut staged = bytes.to_vec();
    staged[5] = (staged[5] & 0xfc) | target.value();
    Ok(staged)
}

pub fn trigger_value(ap_data_byte1: u8) -> u8 {
    (ap_data_byte1 & 0xfc) | 0x01
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

pub(crate) fn validate_writable_variable(variable: &FirmwareVariable) -> Result<()> {
    if variable.bytes.len() != EXPECTED_VARIABLE_LENGTH {
        bail!(
            "refusing write: expected a {EXPECTED_VARIABLE_LENGTH}-byte \
             {MSI_VARIABLE_NAME} value, found {}",
            variable.bytes.len()
        );
    }

    if variable.attributes != EXPECTED_VARIABLE_ATTRIBUTES {
        bail!(
            "refusing write: expected UEFI attributes 0x{EXPECTED_VARIABLE_ATTRIBUTES:08X}, \
             found 0x{:08X}",
            variable.attributes
        );
    }

    Ok(())
}

pub fn collect_snapshot(allow_unknown_hardware: bool) -> Result<StateSnapshot> {
    let mut machine = query_machine()?;
    let profile = matching_profile(&machine);

    // Set this from the profile list rather than relying on a single hard-coded device.
    machine.expected_hardware = profile.is_some();

    if profile.is_none() && !allow_unknown_hardware {
        bail!(
            "unsupported hardware: model '{}', board '{}', BIOS '{}'; \
             use the explicit override only for a deliberate read-only capture",
            machine.model,
            machine.board,
            machine.bios
        );
    }

    let firmware = AvailableSection::from_result(
        read_msi_variable().and_then(|variable| FirmwareInfo::decode(&variable)),
    );
    let wmi_ap = AvailableSection::from_result(MsiAcpi::connect().and_then(|wmi| wmi.get_ap()));
    let secure_boot = AvailableSection::from_result(secure_boot_enabled());

    Ok(StateSnapshot {
        schema_version: 1,
        captured_at: Local::now().to_rfc3339(),
        elevated: is_elevated().unwrap_or(false),
        machine,
        secure_boot,
        registry: platform::query_registry_state(),
        firmware,
        wmi_ap,
        display_devices: query_display_devices()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> FirmwareVariable {
        let mut bytes = vec![0u8; EXPECTED_VARIABLE_LENGTH];
        bytes[5] = 0x3a;

        FirmwareVariable {
            bytes,
            attributes: EXPECTED_VARIABLE_ATTRIBUTES,
        }
    }

    #[test]
    fn matches_supported_hardware_profile() {
        let machine = MachineInfo {
            manufacturer: "MSI".to_string(),
            model: "Vector 16 HX AI A2XWIG".to_string(),
            board: "MS-15M3".to_string(),
            board_revision: String::new(),
            bios: "E15M3IMS.116".to_string(),
            expected_hardware: false,
        };

        assert!(matching_profile(&machine).is_some());
    }

    #[test]
    fn rejects_non_matching_hardware_profile() {
        let machine = MachineInfo {
            manufacturer: "MSI".to_string(),
            model: "Unknown model".to_string(),
            board: "MS-15M3".to_string(),
            board_revision: String::new(),
            bios: "E15M3IMS.116".to_string(),
            expected_hardware: false,
        };

        assert!(matching_profile(&machine).is_none());
    }

    fn machine(model: &str, board: &str, bios: &str) -> MachineInfo {
        MachineInfo {
            manufacturer: "MSI".to_string(),
            model: model.to_string(),
            board: board.to_string(),
            board_revision: String::new(),
            bios: bios.to_string(),
            expected_hardware: false,
        }
    }

    #[test]
    fn lists_every_validated_bios_for_a_model_and_board() {
        for bios in ["E15M3IMS.116", "E15M3IMS.10E", "E15M3IMS.999"] {
            let machine = machine("Vector 16 HX AI A2XWIG", "MS-15M3", bios);
            assert_eq!(
                validated_bios_versions(&machine),
                ["E15M3IMS.116", "E15M3IMS.10E"]
            );
        }

        let machine = machine("Stealth 16Studio A13VG", "MS-15F2", "E15F2IMS.999");
        assert_eq!(validated_bios_versions(&machine), ["E15F2IMS.10F"]);
    }

    #[test]
    fn every_profile_passes_both_the_switching_and_debug_gates() {
        for profile in HARDWARE_PROFILES {
            let machine = machine(profile.model, profile.board, profile.bios);
            assert!(validated_bios_versions(&machine).contains(&profile.bios));
            assert_eq!(matching_profile(&machine), Some(profile));
        }
    }

    #[test]
    fn unlisted_bios_on_a_recognized_board_fails_only_the_bios_gate() {
        let machine = machine("Vector 16 HX AI A2XWIG", "MS-15M3", "E15M3IMS.999");
        let validated = validated_bios_versions(&machine);
        assert!(!validated.is_empty());
        assert!(!validated.contains(&machine.bios.as_str()));
        assert!(matching_profile(&machine).is_none());
    }

    #[test]
    fn unrecognized_model_or_board_has_no_validated_bios() {
        for (model, board) in [
            ("Vector 16 HX AI A2XWIG", "MS-0000"),
            ("Unknown model", "MS-15M3"),
            ("Vector 16 HX AI A2XWIG", "MS-15F2"),
        ] {
            let machine = machine(model, board, "E15M3IMS.10E");
            assert!(validated_bios_versions(&machine).is_empty());
        }
    }

    #[test]
    fn decodes_observed_integrated_baseline() {
        let info = FirmwareInfo::decode(&baseline()).unwrap();
        assert_eq!(info.selected_target_mode, Some(Mode::Integrated));
        assert_eq!(info.current_mode, Some(Mode::Integrated));
        assert_eq!(info.available_modes(), Mode::ALL);
    }

    #[test]
    fn stages_only_target_bits() {
        let before = baseline();

        for (mode, expected) in [
            (Mode::MsHybrid, 0x38),
            (Mode::Discrete, 0x39),
            (Mode::Integrated, 0x3a),
        ] {
            let after = stage_target(&before.bytes, mode).unwrap();
            assert_eq!(after[5], expected);
            assert_eq!(&after[..5], &before.bytes[..5]);
            assert_eq!(&after[6..], &before.bytes[6..]);
        }
    }

    #[test]
    fn trigger_preserves_upper_six_bits() {
        for value in 0u8..=u8::MAX {
            let trigger = trigger_value(value);
            assert_eq!(trigger & 0xfc, value & 0xfc);
            assert_eq!(trigger & 0x03, 0x01);
        }
    }

    #[test]
    fn writable_variable_requires_exact_layout_and_attributes() {
        let mut variable = baseline();
        variable.bytes.pop();
        assert!(validate_writable_variable(&variable).is_err());

        let mut variable = baseline();
        variable.attributes ^= 1;
        assert!(validate_writable_variable(&variable).is_err());
    }

    #[test]
    fn decodes_get_ap_success_and_ready_flag() {
        let state = decode_ap_state(&[1, 0xaa, 0x82]).unwrap();
        assert_eq!(state.flag, 1);
        assert_eq!(state.data_byte1, 0x82);
        assert!(state.apply_ready);
    }

    #[test]
    fn rejects_unsuccessful_or_short_get_ap_package() {
        assert!(decode_ap_state(&[0, 0, 0]).is_err());
        assert!(decode_ap_state(&[1, 0]).is_err());
    }
}
