//! Reads this machine's id from the OS and turns it into a short, stable
//! device id. The raw machine id never leaves the machine: only a hash of it
//! with a fixed prefix does (see machine-id(5)).

use sha2::{Digest, Sha256};

// fixed prefix that makes the hash specific to deskpane; v1 lets a later format change ids on purpose
const ID_HASH_PREFIX: &str = "deskpane-device-id-v1:";

/*
 * reads this machine's id from the OS
 *
 * returns: the id (lowercase), or None when the OS has none or it looks wrong
 */
#[cfg(target_os = "linux")]
pub fn read_machine_id() -> Option<String> {
    // systemd's file first, then the older D-Bus copy
    let paths = ["/etc/machine-id", "/var/lib/dbus/machine-id"];
    for path in paths {
        let read = std::fs::read_to_string(path);
        let text = match read {
            Ok(t) => t,
            Err(_) => continue,
        };

        let parsed = parse_linux_machine_id(&text);
        match parsed {
            Some(id) => return Some(id),
            None => continue,
        }
    }
    None
}

/*
 * reads this machine's id from the OS (MachineGuid in the registry, via reg.exe)
 *
 * returns: the id (lowercase), or None when it cannot be read or looks wrong
 */
#[cfg(windows)]
pub fn read_machine_id() -> Option<String> {
    // absolute path, never a search of PATH or the current folder
    let reg_path = reg_exe_path(read_system_root());
    let ran = std::process::Command::new(reg_path)
        .args([
            "query",
            "HKLM\\SOFTWARE\\Microsoft\\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output();
    let output = match ran {
        Ok(o) => o,
        Err(_) => return None,
    };

    let text = String::from_utf8_lossy(&output.stdout);
    parse_windows_reg_output(&text)
}

// %SystemRoot%, e.g. C:\Windows; None when it is not set
#[cfg(windows)]
fn read_system_root() -> Option<String> {
    let read = std::env::var("SystemRoot");
    match read {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

/*
 * absolute path of reg.exe: <SystemRoot>\System32\reg.exe, with C:\Windows
 * when SystemRoot is missing, empty or not an absolute drive path
 * (used on Windows only; tests run it everywhere)
 *
 * system_root: the SystemRoot environment variable, if set
 */
#[cfg_attr(not(windows), allow(dead_code))]
fn reg_exe_path(system_root: Option<String>) -> String {
    let given = match system_root {
        Some(value) => value,
        None => String::new(),
    };

    let mut root = "C:\\Windows".to_string();
    if is_absolute_drive_path(&given) {
        root = given.trim_end_matches('\\').to_string();
    }
    format!("{root}\\System32\\reg.exe")
}

// true for a path like C:\Windows (drive letter, colon, backslash)
#[cfg_attr(not(windows), allow(dead_code))]
fn is_absolute_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

// other systems have no machine id source yet: the user passes --device-id
#[cfg(not(any(target_os = "linux", windows)))]
pub fn read_machine_id() -> Option<String> {
    None
}

/*
 * checks the text of /etc/machine-id
 *
 * text: the whole file
 * returns: the id trimmed and lowercased, or None unless it is exactly 32 hex chars
 * (used by read_machine_id on Linux only; tests run it everywhere)
 */
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_linux_machine_id(text: &str) -> Option<String> {
    let id = text.trim().to_ascii_lowercase();
    if id.len() != 32 {
        return None;
    }

    for c in id.chars() {
        if !c.is_ascii_hexdigit() {
            return None;
        }
    }

    Some(id)
}

/*
 * finds MachineGuid in the output of
 * reg query HKLM\SOFTWARE\Microsoft\Cryptography /v MachineGuid
 *
 * text: everything reg.exe printed
 * returns: the GUID lowercased, or None when it is missing or not a GUID
 * (used by read_machine_id on Windows only; tests run it everywhere)
 */
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_windows_reg_output(text: &str) -> Option<String> {
    // the value line is: MachineGuid    REG_SZ    <guid>
    for line in text.lines() {
        let mut words = line.split_whitespace();

        let name = words.next();
        match name {
            Some("MachineGuid") => {}
            _ => continue,
        }

        // skip the type (REG_SZ), then the value
        words.next();
        let value = words.next();
        let guid = match value {
            Some(v) => v.to_ascii_lowercase(),
            None => return None,
        };

        if is_guid(&guid) {
            return Some(guid);
        }
        return None;
    }
    None
}

// true for 8-4-4-4-12 hex digits with dashes, e.g. 3f2504e0-4f89-11d3-9a0c-0305e82c3301
#[cfg_attr(not(windows), allow(dead_code))]
fn is_guid(text: &str) -> bool {
    if text.len() != 36 {
        return false;
    }

    // char_indices gives each character with its byte position
    for (position, c) in text.char_indices() {
        let dash_expected;
        if position == 8 || position == 13 || position == 18 || position == 23 {
            dash_expected = true;
        } else {
            dash_expected = false;
        }

        if dash_expected && c != '-' {
            return false;
        }
        if !dash_expected && !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/*
 * turns a machine id into the default device id: "dp-" plus the first 8 hex
 * chars of SHA-256 over a fixed deskpane prefix and the machine id
 *
 * machine_id: the raw id (trimmed and lowercased here)
 * returns: e.g. dp-3f9a0c41
 */
pub fn derive_device_id(machine_id: &str) -> String {
    let normalised = machine_id.trim().to_ascii_lowercase();
    let input = format!("{ID_HASH_PREFIX}{normalised}");

    let digest = Sha256::digest(input.as_bytes());
    let hex = format!("{digest:x}");

    format!("dp-{}", &hex[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon::contract::check_device_id;

    const LINUX_ID: &str = "0123456789abcdef0123456789abcdef";

    // ---- Linux /etc/machine-id ----

    #[test]
    fn linux_machine_id_is_trimmed_and_lowercased() {
        let parsed = parse_linux_machine_id("  0123456789ABCDEF0123456789abcdef\n");
        assert_eq!(parsed, Some(LINUX_ID.to_string()));
    }

    #[test]
    fn linux_machine_id_empty_file_is_missing() {
        assert_eq!(parse_linux_machine_id(""), None);
        assert_eq!(parse_linux_machine_id("\n"), None);
    }

    #[test]
    fn linux_machine_id_wrong_length_is_missing() {
        assert_eq!(parse_linux_machine_id("0123456789abcdef"), None);
        assert_eq!(parse_linux_machine_id(&format!("{LINUX_ID}0")), None);
    }

    #[test]
    fn linux_machine_id_non_hex_is_missing() {
        assert_eq!(
            parse_linux_machine_id("0123456789abcdef0123456789abcdeg"),
            None
        );
        // "uninitialized" is what systemd writes before first boot finishes
        assert_eq!(parse_linux_machine_id("uninitialized"), None);
    }

    // ---- Windows reg.exe output ----

    #[test]
    fn windows_reg_output_yields_the_guid() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    MachineGuid    REG_SZ    3F2504E0-4F89-11D3-9A0C-0305E82C3301\r\n\r\n";
        let parsed = parse_windows_reg_output(output);
        assert_eq!(
            parsed,
            Some("3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string())
        );
    }

    #[test]
    fn windows_reg_output_without_machineguid_is_missing() {
        let output =
            "ERROR: The system was unable to find the specified registry key or value.\r\n";
        assert_eq!(parse_windows_reg_output(output), None);
        assert_eq!(parse_windows_reg_output(""), None);
    }

    #[test]
    fn windows_reg_output_with_malformed_guid_is_missing() {
        let short = "    MachineGuid    REG_SZ    3f2504e0-4f89-11d3-9a0c\r\n";
        assert_eq!(parse_windows_reg_output(short), None);
        let not_hex = "    MachineGuid    REG_SZ    zf2504e0-4f89-11d3-9a0c-0305e82c3301\r\n";
        assert_eq!(parse_windows_reg_output(not_hex), None);
        let no_dashes = "    MachineGuid    REG_SZ    3f2504e04f8911d39a0c0305e82c33010000\r\n";
        assert_eq!(parse_windows_reg_output(no_dashes), None);
        let no_value = "    MachineGuid    REG_SZ\r\n";
        assert_eq!(parse_windows_reg_output(no_value), None);
    }

    // ---- reg.exe path ----

    #[test]
    fn reg_exe_path_is_built_from_system_root() {
        let path = reg_exe_path(Some("D:\\Win".to_string()));
        assert_eq!(path, "D:\\Win\\System32\\reg.exe");
    }

    #[test]
    fn reg_exe_path_drops_a_trailing_backslash_on_system_root() {
        let path = reg_exe_path(Some("C:\\Windows\\".to_string()));
        assert_eq!(path, "C:\\Windows\\System32\\reg.exe");
    }

    #[test]
    fn reg_exe_path_without_system_root_falls_back_to_c_windows() {
        assert_eq!(reg_exe_path(None), "C:\\Windows\\System32\\reg.exe");
    }

    #[test]
    fn reg_exe_path_with_empty_or_relative_system_root_falls_back() {
        // a relative root would be searched from the current folder: never use it
        assert_eq!(
            reg_exe_path(Some(String::new())),
            "C:\\Windows\\System32\\reg.exe"
        );
        assert_eq!(
            reg_exe_path(Some("Windows".to_string())),
            "C:\\Windows\\System32\\reg.exe"
        );
    }

    // ---- derived id ----

    #[test]
    fn derived_id_has_dp_prefix_and_8_hex_chars() {
        let id = derive_device_id(LINUX_ID);

        assert_eq!(id.len(), 11);
        assert!(id.starts_with("dp-"));
        for c in id[3..].chars() {
            assert!(
                c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
                "bad char {c}"
            );
        }
    }

    #[test]
    fn derived_id_is_stable_for_the_same_machine_id() {
        // fixed input, fixed output: a change of algorithm breaks this on purpose
        assert_eq!(derive_device_id(LINUX_ID), "dp-e2f21a97");
        assert_eq!(
            derive_device_id("3f2504e0-4f89-11d3-9a0c-0305e82c3301"),
            "dp-f30950ac"
        );
    }

    #[test]
    fn derived_id_ignores_case_and_surrounding_space() {
        let upper = format!("  {}\n", LINUX_ID.to_uppercase());
        assert_eq!(derive_device_id(&upper), "dp-e2f21a97");
    }

    #[test]
    fn derived_id_differs_for_different_machine_ids() {
        let first = derive_device_id(LINUX_ID);
        let second = derive_device_id("fedcba9876543210fedcba9876543210");
        assert_ne!(first, second);
        assert_eq!(second, "dp-6f3f7e22");
    }

    #[test]
    fn derived_id_passes_the_contract_id_rule() {
        let id = derive_device_id(LINUX_ID);
        assert!(check_device_id(&id).is_ok());
    }

    #[test]
    fn derived_id_does_not_contain_the_raw_machine_id() {
        let id = derive_device_id(LINUX_ID);
        assert!(!LINUX_ID.contains(&id[3..]));
    }
}
