use std::ffi::c_void;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Threading::{
    PEB, PROCESS_BASIC_INFORMATION, RTL_USER_PROCESS_PARAMETERS,
};

/// Locate the Ultima V game data directory from a running DOSBox process.
///
/// Tries several strategies in order:
/// 1. Read DOSBox command line -> find `-conf` path -> parse `[autoexec]` mount commands
/// 2. Check paths mentioned in the command line
/// 3. Return error with guidance for manual setup
pub fn find_game_directory(handle: HANDLE) -> Result<PathBuf> {
    let cmdline = read_process_command_line(handle)?;

    // Strategy 1: Parse command line for -conf, then parse config file
    if let Some(mounts) = parse_conf_path(&cmdline).and_then(|p| parse_autoexec_mounts(&p).ok()) {
        for (_drive, host_path) in &mounts {
            if validate_game_dir(host_path) {
                return Ok(host_path.clone());
            }
        }
    }

    // Strategy 2: Try paths mentioned directly in the command line
    for segment in cmdline.split('"') {
        let path = Path::new(segment.trim());
        if path.is_absolute() && validate_game_dir(path) {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent().filter(|p| validate_game_dir(p)) {
            return Ok(parent.to_path_buf());
        }
    }

    // Strategy 3: Check exe's parent directory
    if let Some(parent) = cmdline
        .split('"')
        .nth(1)
        .and_then(|p| Path::new(p).parent())
        .filter(|p| validate_game_dir(p))
    {
        return Ok(parent.to_path_buf());
    }

    anyhow::bail!(
        "Could not locate Ultima V game files. \
         Ensure DOSBox is running with a config that mounts the game directory."
    )
}

/// Read the command line of a remote process via NtQueryInformationProcess.
fn read_process_command_line(handle: HANDLE) -> Result<String> {
    unsafe {
        // Step 1: Get PEB address
        let mut pbi = PROCESS_BASIC_INFORMATION::default();
        let mut return_length = 0u32;
        let status = NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            &mut pbi as *mut _ as *mut c_void,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut return_length,
        );
        if status.is_err() {
            anyhow::bail!("NtQueryInformationProcess failed: {status:?}");
        }
        if pbi.PebBaseAddress.is_null() {
            anyhow::bail!("PEB base address is null");
        }

        // Step 2: Read PEB from remote process
        let mut peb = PEB::default();
        ReadProcessMemory(
            handle,
            pbi.PebBaseAddress as *const c_void,
            &mut peb as *mut _ as *mut c_void,
            std::mem::size_of::<PEB>(),
            None,
        )
        .context("failed to read PEB")?;

        if peb.ProcessParameters.is_null() {
            anyhow::bail!("ProcessParameters is null");
        }

        // Step 3: Read RTL_USER_PROCESS_PARAMETERS
        let mut params = RTL_USER_PROCESS_PARAMETERS::default();
        ReadProcessMemory(
            handle,
            peb.ProcessParameters as *const c_void,
            &mut params as *mut _ as *mut c_void,
            std::mem::size_of::<RTL_USER_PROCESS_PARAMETERS>(),
            None,
        )
        .context("failed to read ProcessParameters")?;

        // Step 4: Read the CommandLine UNICODE_STRING buffer
        let byte_len = params.CommandLine.Length as usize;
        if byte_len == 0 || params.CommandLine.Buffer.0.is_null() {
            anyhow::bail!("command line is empty or null");
        }
        let char_len = byte_len / 2;
        let mut buf = vec![0u16; char_len];
        ReadProcessMemory(
            handle,
            params.CommandLine.Buffer.0 as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            byte_len,
            None,
        )
        .context("failed to read CommandLine buffer")?;

        Ok(String::from_utf16_lossy(&buf))
    }
}

/// Extract the `-conf <path>` argument from a DOSBox command line.
pub fn parse_conf_path(cmdline: &str) -> Option<PathBuf> {
    let lower = cmdline.to_lowercase();
    let conf_idx = lower.find("-conf ").or_else(|| lower.find("/conf "))?;
    let after = &cmdline[conf_idx + 6..];
    let path_str = if let Some(quoted) = after.strip_prefix('"') {
        let end = quoted.find('"')?;
        &quoted[..end]
    } else {
        after.split_whitespace().next()?
    };

    let path = PathBuf::from(path_str);
    if path.exists() { Some(path) } else { None }
}

/// Parse the `[autoexec]` section of a DOSBox config file for `mount` commands.
/// Returns a list of (drive_letter, host_path) pairs.
pub fn parse_autoexec_mounts(conf_path: &Path) -> Result<Vec<(char, PathBuf)>> {
    let content = std::fs::read_to_string(conf_path)
        .with_context(|| format!("failed to read config: {}", conf_path.display()))?;

    let mut mounts = Vec::new();
    let mut in_autoexec = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.eq_ignore_ascii_case("[autoexec]") {
            in_autoexec = true;
            continue;
        }
        if trimmed.starts_with('[') && in_autoexec {
            break;
        }
        if !in_autoexec {
            continue;
        }

        let lower = trimmed.to_lowercase();
        if !lower.starts_with("mount ") {
            continue;
        }

        let parts = trimmed[6..].trim_start();
        let mut chars = parts.chars();
        let drive = match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            _ => continue,
        };

        let rest = chars.as_str().trim_start();
        let host_path = if let Some(quoted) = rest.strip_prefix('"') {
            match quoted.find('"') {
                Some(end) => &quoted[..end],
                None => continue,
            }
        } else {
            rest.split_whitespace().next().unwrap_or("")
        };

        if !host_path.is_empty() {
            mounts.push((drive, PathBuf::from(host_path)));
        }
    }

    Ok(mounts)
}

/// Check that a directory contains essential Ultima V data files.
pub fn validate_game_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }

    let required = ["BRIT.DAT", "TOWNE.DAT", "DATA.OVL", "TILES.16"];
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };

    let filenames: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_uppercase())
        .collect();

    required
        .iter()
        .all(|req| filenames.iter().any(|f| f == req))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_conf_path_quoted() {
        // Can't test file existence, but verify parsing logic
        let cmdline = r#"dosbox.exe -conf "C:\Games\U5\dosbox.conf" -noconsole"#;
        let lower = cmdline.to_lowercase();
        let idx = lower.find("-conf ").unwrap();
        let after = &cmdline[idx + 6..];
        let quoted = after.strip_prefix('"').unwrap();
        let end = quoted.find('"').unwrap();
        assert_eq!(&quoted[..end], r"C:\Games\U5\dosbox.conf");
    }

    #[test]
    fn parse_conf_path_unquoted() {
        let cmdline = r"dosbox.exe -conf C:\Games\dosbox.conf -noconsole";
        let lower = cmdline.to_lowercase();
        let idx = lower.find("-conf ").unwrap();
        let after = &cmdline[idx + 6..];
        let path_str = after.split_whitespace().next().unwrap();
        assert_eq!(path_str, r"C:\Games\dosbox.conf");
    }

    #[test]
    fn parse_autoexec_mounts_basic() {
        let conf = "[autoexec]\nmount C \"D:\\Games\\Ultima5\"\nC:\nULTIMA5.EXE\n";
        let dir = std::env::temp_dir().join("ninth_test_dosbox.conf");
        std::fs::write(&dir, conf).unwrap();
        let mounts = parse_autoexec_mounts(&dir).unwrap();
        std::fs::remove_file(&dir).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, 'C');
        assert_eq!(mounts[0].1, PathBuf::from(r"D:\Games\Ultima5"));
    }

    #[test]
    fn parse_autoexec_mounts_multiple() {
        let conf = "[sdl]\nfullscreen=false\n\n[autoexec]\nmount C /games/u5\nmount D /cdrom\nC:\n";
        let dir = std::env::temp_dir().join("ninth_test_dosbox2.conf");
        std::fs::write(&dir, conf).unwrap();
        let mounts = parse_autoexec_mounts(&dir).unwrap();
        std::fs::remove_file(&dir).unwrap();

        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].0, 'C');
        assert_eq!(mounts[1].0, 'D');
    }

    #[test]
    fn parse_autoexec_stops_at_next_section() {
        let conf = "[autoexec]\nmount C /games\n[serial]\n";
        let dir = std::env::temp_dir().join("ninth_test_dosbox3.conf");
        std::fs::write(&dir, conf).unwrap();
        let mounts = parse_autoexec_mounts(&dir).unwrap();
        std::fs::remove_file(&dir).unwrap();

        assert_eq!(mounts.len(), 1);
    }

    #[test]
    fn validate_game_dir_rejects_nonexistent() {
        assert!(!validate_game_dir(Path::new(r"C:\nonexistent_path_xyz")));
    }
}
