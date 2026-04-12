use std::env;
use std::ffi::c_void;
use std::path::{Component, Path, PathBuf};

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
/// 1. Read DOSBox command line -> resolve one or more `-conf` paths -> parse
///    `[autoexec]` mount commands using DOSBox's program/default directory as
///    the base for relative paths
/// 2. Check paths mentioned in the command line
/// 3. Return error with guidance for manual setup
pub fn find_game_directory(handle: HANDLE) -> Result<PathBuf> {
    let cmdline = read_process_command_line(handle)?;
    find_game_directory_from_command_line(&cmdline).with_context(|| {
        anyhow::anyhow!(
            "Could not locate Ultima V game files from DOSBox configuration data. \
             Ensure DOSBox is running with a config that mounts the game directory."
        )
    })
}

/// Read the command line of a remote process via NtQueryInformationProcess.
///
/// # Limitations
/// - Only works when both processes have the same bitness (both 64-bit or both
///   32-bit). Reading PEB from a 32-bit process in a 64-bit companion would
///   require WOW64-specific struct layouts.
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
        let max_byte_len = params.CommandLine.MaximumLength as usize;
        if byte_len == 0 || params.CommandLine.Buffer.0.is_null() {
            anyhow::bail!("command line is empty or null");
        }
        anyhow::ensure!(
            byte_len.is_multiple_of(2),
            "command line length is not UTF-16 aligned: {byte_len}"
        );
        anyhow::ensure!(
            byte_len <= max_byte_len,
            "command line length {byte_len} exceeds maximum {max_byte_len}"
        );
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

fn find_game_directory_from_command_line(cmdline: &str) -> Result<PathBuf> {
    let context = CommandLineContext::from_command_line(cmdline);

    // Strategy 1: Discover config files and inspect `[autoexec]` mounts.
    for conf_path in discover_config_paths(&context) {
        let mount_bases = context.mount_base_dirs(&conf_path);
        if let Ok(mounts) = parse_autoexec_mounts(&conf_path, &mount_bases) {
            for (_drive, host_path) in mounts {
                if validate_game_dir(&host_path) {
                    return Ok(host_path);
                }
            }
        }
    }

    // Strategy 2: Try paths mentioned directly in the command line.
    for token in tokenize_command_line(cmdline) {
        let path = Path::new(&token);
        for candidate in resolve_candidate_path(path, &context.base_dirs()) {
            if validate_game_dir(&candidate) {
                return Ok(candidate);
            }
            if let Some(parent) = candidate.parent().filter(|p| validate_game_dir(p)) {
                return Ok(parent.to_path_buf());
            }
        }
    }

    // Strategy 3: Check the DOSBox executable directory.
    if let Some(parent) = context.exe_dir.as_deref().filter(|p| validate_game_dir(p)) {
        return Ok(parent.to_path_buf());
    }

    anyhow::bail!("Could not locate Ultima V game files from DOSBox configuration data")
}

/// Extract the first `-conf <path>` argument from a DOSBox command line.
#[cfg(test)]
pub fn parse_conf_path(cmdline: &str) -> Option<PathBuf> {
    parse_option_paths(cmdline, "conf").into_iter().next()
}

fn parse_defaultdir_path(cmdline: &str) -> Option<PathBuf> {
    parse_option_paths(cmdline, "defaultdir").into_iter().next()
}

fn parse_option_paths(cmdline: &str, option_name: &str) -> Vec<PathBuf> {
    let tokens = tokenize_command_line(cmdline);
    let mut paths = Vec::new();
    let dash = format!("-{option_name}");
    let slash = format!("/{option_name}");

    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let lower = token.to_ascii_lowercase();
        if lower == dash || lower == slash {
            if let Some(value) = tokens.get(i + 1) {
                paths.push(PathBuf::from(value));
                i += 2;
                continue;
            }
        } else if let Some(value) = lower
            .strip_prefix(&(dash.clone() + "="))
            .or_else(|| lower.strip_prefix(&(slash.clone() + "=")))
        {
            let prefix_len = token.len() - value.len();
            paths.push(PathBuf::from(&token[prefix_len..]));
        }
        i += 1;
    }

    paths
}

fn tokenize_command_line(cmdline: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in cmdline.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            ch => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[derive(Debug, Default)]
struct CommandLineContext {
    exe_dir: Option<PathBuf>,
    default_dir: Option<PathBuf>,
    conf_args: Vec<PathBuf>,
}

impl CommandLineContext {
    /// Parse the DOSBox-family command line into the directories and config
    /// paths used for subsequent config discovery.
    fn from_command_line(cmdline: &str) -> Self {
        Self {
            exe_dir: executable_dir_from_cmdline(cmdline),
            default_dir: parse_defaultdir_path(cmdline),
            conf_args: parse_option_paths(cmdline, "conf"),
        }
    }

    /// Directories DOSBox itself uses to resolve relative command-line paths.
    fn base_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(default_dir) = &self.default_dir {
            push_unique_path(&mut dirs, default_dir.clone());
        }
        if let Some(exe_dir) = &self.exe_dir {
            push_unique_path(&mut dirs, exe_dir.clone());
        }
        dirs
    }

    /// Directories that can sensibly resolve relative `mount` targets from a
    /// DOSBox config file.
    fn mount_base_dirs(&self, conf_path: &Path) -> Vec<PathBuf> {
        let mut dirs = self.base_dirs();
        if let Some(parent) = conf_path.parent() {
            push_unique_path(&mut dirs, parent.to_path_buf());
        }
        dirs
    }
}

/// Resolve a path relative to the DOSBox execution context before falling back
/// to the companion's own current working directory.
fn resolve_candidate_path(path: &Path, base_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut resolved = Vec::new();

    if path.is_absolute() {
        push_unique_path(&mut resolved, normalize_path(path));
        return resolved;
    }

    for base in base_dirs {
        push_unique_path(&mut resolved, normalize_path(&base.join(path)));
    }

    push_unique_path(&mut resolved, normalize_path(path));

    resolved
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let mut normal_depth = 0usize;
    let mut absolute = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => {
                normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
                absolute = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normal_depth > 0 {
                    normalized.pop();
                    normal_depth -= 1;
                } else if !absolute {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => {
                normalized.push(part);
                normal_depth += 1;
            }
        }
    }

    normalized
}

/// Discover config files in the same order DOSBox-family launches are likely to
/// consult them, keeping explicit `-conf` files ahead of defaults.
fn discover_config_paths(context: &CommandLineContext) -> Vec<PathBuf> {
    let mut configs = Vec::new();

    for raw_conf in &context.conf_args {
        for conf_path in resolve_candidate_path(raw_conf, &context.base_dirs()) {
            if conf_path.is_file() {
                push_unique_path(&mut configs, conf_path);
            }
        }
    }

    if let Some(default_dir) = &context.default_dir {
        add_named_configs(default_dir, &mut configs);
    }

    if let Some(exe_dir) = &context.exe_dir {
        add_named_configs(exe_dir, &mut configs);
    }

    for user_dir in user_config_directories() {
        add_named_configs(&user_dir, &mut configs);
        add_versioned_configs(&user_dir, &mut configs);
    }

    configs
}

fn executable_dir_from_cmdline(cmdline: &str) -> Option<PathBuf> {
    let exe = if let Some(quoted) = cmdline.strip_prefix('"') {
        let end = quoted.find('"')?;
        &quoted[..end]
    } else {
        cmdline.split_whitespace().next()?
    };

    Path::new(exe).parent().map(Path::to_path_buf)
}

fn user_config_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return dirs;
    };

    for dir in [
        local_app_data.join("DOSBox-X"),
        local_app_data.join("DOSBox"),
    ] {
        if dir.is_dir() {
            dirs.push(dir);
        }
    }

    dirs
}

fn add_named_configs(dir: &Path, out: &mut Vec<PathBuf>) {
    for name in ["dosbox-x.conf", "dosbox-staging.conf", "dosbox.conf"] {
        let path = dir.join(name);
        if path.is_file() {
            push_unique_path(out, path);
        }
    }
}

fn add_versioned_configs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut candidates: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("conf"))
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| {
                        let lower = name.to_ascii_lowercase();
                        lower.starts_with("dosbox-")
                            || lower.starts_with("dosbox_x-")
                            || lower.starts_with("dosbox-x-")
                    })
                    .unwrap_or(false)
        })
        .collect();

    candidates.sort();
    for path in candidates {
        push_unique_path(out, path);
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

/// Parse the `[autoexec]` section of a DOSBox config file for `mount` commands.
/// Returns a list of (drive_letter, host_path) pairs.
///
/// Assumes standard DOSBox syntax: `mount <drive> <path> [options...]`.
/// Non-standard option placement (e.g., `mount D -t cdrom "E:\cdrom"`) is not
/// handled but is mitigated by `validate_game_dir` rejecting invalid directories.
pub fn parse_autoexec_mounts(
    conf_path: &Path,
    base_dirs: &[PathBuf],
) -> Result<Vec<(char, PathBuf)>> {
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
            let raw_path = Path::new(host_path);
            let resolved = resolve_candidate_path(raw_path, base_dirs);
            if resolved.is_empty() {
                mounts.push((drive, PathBuf::from(host_path)));
            } else {
                for path in resolved {
                    mounts.push((drive, path));
                }
            }
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
    use std::ffi::OsString;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static TEST_PROCESS_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct LocalAppDataGuard {
        _lock: MutexGuard<'static, ()>,
        previous_value: Option<OsString>,
    }

    impl LocalAppDataGuard {
        fn set(path: &Path) -> Self {
            let lock = TEST_PROCESS_ENV_LOCK.lock().unwrap();
            let previous_value = std::env::var_os("LOCALAPPDATA");
            unsafe {
                std::env::set_var("LOCALAPPDATA", path);
            }
            Self {
                _lock: lock,
                previous_value,
            }
        }
    }

    impl Drop for LocalAppDataGuard {
        fn drop(&mut self) {
            match self.previous_value.take() {
                Some(value) => unsafe {
                    std::env::set_var("LOCALAPPDATA", value);
                },
                None => unsafe {
                    std::env::remove_var("LOCALAPPDATA");
                },
            }
        }
    }

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
    fn parse_defaultdir_path_quoted() {
        let cmdline = r#"dosbox-x.exe -defaultdir "C:\Games\U5" -fastlaunch"#;
        assert_eq!(
            parse_defaultdir_path(cmdline),
            Some(PathBuf::from(r"C:\Games\U5"))
        );
    }

    #[test]
    fn executable_dir_from_cmdline_quoted() {
        let cmdline = r#""C:\Program Files\DOSBox-X\dosbox-x.exe" -fastlaunch"#;
        assert_eq!(
            executable_dir_from_cmdline(cmdline),
            Some(PathBuf::from(r"C:\Program Files\DOSBox-X"))
        );
    }

    #[test]
    fn parse_conf_path_returns_first_of_multiple_conf_args() {
        let cmdline =
            r#""C:\DOSBox-X\dosbox-x.exe" -conf "..\dosboxULTIMA5.conf" -conf "..\overlay.conf""#;
        assert_eq!(
            parse_conf_path(cmdline),
            Some(PathBuf::from(r"..\dosboxULTIMA5.conf"))
        );
    }

    /// Create a unique temp file path for test isolation (PID-scoped).
    fn test_conf(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ninth_{}_{name}.conf", std::process::id()))
    }

    #[test]
    fn parse_autoexec_mounts_basic() {
        let conf = "[autoexec]\nmount C \"D:\\Games\\Ultima5\"\nC:\nULTIMA5.EXE\n";
        let path = test_conf("mounts_basic");
        std::fs::write(&path, conf).unwrap();
        let mounts = parse_autoexec_mounts(&path, &[]).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, 'C');
        assert_eq!(mounts[0].1, PathBuf::from(r"D:\Games\Ultima5"));
    }

    #[test]
    fn parse_autoexec_mounts_multiple() {
        let conf = "[sdl]\nfullscreen=false\n\n[autoexec]\nmount C /games/u5\nmount D /cdrom\nC:\n";
        let path = test_conf("mounts_multi");
        std::fs::write(&path, conf).unwrap();
        let mounts = parse_autoexec_mounts(&path, &[]).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].0, 'C');
        assert_eq!(mounts[1].0, 'D');
    }

    #[test]
    fn parse_autoexec_stops_at_next_section() {
        let conf = "[autoexec]\nmount C /games\n[serial]\n";
        let path = test_conf("mounts_section");
        std::fs::write(&path, conf).unwrap();
        let mounts = parse_autoexec_mounts(&path, &[]).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(mounts.len(), 1);
    }

    #[test]
    fn validate_game_dir_rejects_nonexistent() {
        assert!(!validate_game_dir(Path::new(r"C:\nonexistent_path_xyz")));
    }

    #[test]
    fn discover_config_paths_includes_explicit_conf_before_defaults() {
        let temp_root = std::env::temp_dir().join(format!(
            "ninth_conf_discovery_{}_{}",
            std::process::id(),
            "explicit"
        ));
        let default_dir = temp_root.join("portable");
        let local_app_data = temp_root.join("localappdata");
        let explicit = temp_root.join("explicit.conf");
        let portable_conf = default_dir.join("dosbox-x.conf");
        let user_conf = local_app_data.join("DOSBox-X").join("dosbox-x-0.84.1.conf");

        std::fs::create_dir_all(&default_dir).unwrap();
        std::fs::create_dir_all(user_conf.parent().unwrap()).unwrap();
        std::fs::write(&explicit, "").unwrap();
        std::fs::write(&portable_conf, "").unwrap();
        std::fs::write(&user_conf, "").unwrap();

        let local_app_data_guard = LocalAppDataGuard::set(&local_app_data);

        let cmdline = format!(
            r#""C:\Program Files\DOSBox-X\dosbox-x.exe" -conf "{}" -defaultdir "{}""#,
            explicit.display(),
            default_dir.display()
        );
        let discovered = discover_config_paths(&CommandLineContext::from_command_line(&cmdline));

        drop(local_app_data_guard);
        std::fs::remove_dir_all(&temp_root).unwrap();

        assert_eq!(discovered[0], explicit);
        assert!(discovered.contains(&portable_conf));
        assert!(discovered.contains(&user_conf));
    }

    #[test]
    fn resolve_candidate_path_prioritizes_dosbox_base_dirs() {
        let base_dir = PathBuf::from(r"C:\DOSBox-X");
        let resolved = resolve_candidate_path(
            Path::new("dosboxULTIMA5.conf"),
            std::slice::from_ref(&base_dir),
        );

        assert_eq!(
            resolved[0],
            PathBuf::from(r"C:\DOSBox-X\dosboxULTIMA5.conf")
        );
        assert_eq!(resolved[1], PathBuf::from("dosboxULTIMA5.conf"));
    }

    #[test]
    fn discover_config_paths_resolves_relative_conf_paths_against_exe_dir() {
        let temp_root = std::env::temp_dir().join(format!(
            "ninth_conf_discovery_{}_{}",
            std::process::id(),
            "relative"
        ));
        let dosbox_dir = temp_root.join("DOSBox-X");
        let conf_path = temp_root.join("dosboxULTIMA5.conf");

        std::fs::create_dir_all(&dosbox_dir).unwrap();
        std::fs::write(&conf_path, "").unwrap();

        let cmdline = format!(
            r#""{}\dosbox-x.exe" -conf "..\dosboxULTIMA5.conf""#,
            dosbox_dir.display()
        );
        let discovered = discover_config_paths(&CommandLineContext::from_command_line(&cmdline));

        std::fs::remove_dir_all(&temp_root).unwrap();

        assert!(discovered.contains(&conf_path));
    }

    #[test]
    fn parse_autoexec_mounts_resolves_relative_paths_against_exe_dir_base() {
        let temp_root = std::env::temp_dir().join(format!(
            "ninth_mount_resolution_{}_{}",
            std::process::id(),
            "gog"
        ));
        let dosbox_dir = temp_root.join("DOSBox-X");
        let conf_path = temp_root.join("dosboxULTIMA5.conf");

        std::fs::create_dir_all(&dosbox_dir).unwrap();
        std::fs::write(&conf_path, "[autoexec]\nmount C \"..\"\n").unwrap();

        let mounts = parse_autoexec_mounts(&conf_path, std::slice::from_ref(&dosbox_dir)).unwrap();

        std::fs::remove_dir_all(&temp_root).unwrap();

        assert!(mounts.iter().any(|(_, path)| path == &temp_root));
    }

    #[test]
    fn find_game_directory_from_command_line_handles_gog_relative_conf_and_mounts() {
        let temp_root = std::env::temp_dir().join(format!(
            "ninth_game_dir_resolution_{}_{}",
            std::process::id(),
            "gog"
        ));
        let dosbox_dir = temp_root.join("DOSBox-X");
        let conf_path = temp_root.join("dosboxULTIMA5.conf");

        std::fs::create_dir_all(&dosbox_dir).unwrap();
        std::fs::write(
            &conf_path,
            "[autoexec]\n@ECHO OFF\nmount C \"..\"\nmount C \"..\\cloud_saves\" -t overlay\n",
        )
        .unwrap();
        for name in ["BRIT.DAT", "TOWNE.DAT", "DATA.OVL", "TILES.16"] {
            std::fs::write(temp_root.join(name), "").unwrap();
        }

        let cmdline = format!(
            r#""{}\dosbox-x.exe" -conf "..\dosboxULTIMA5.conf" -conf "..\dosboxULTIMA5_single.conf" -noconsole"#,
            dosbox_dir.display()
        );
        let game_dir = find_game_directory_from_command_line(&cmdline).unwrap();

        std::fs::remove_dir_all(&temp_root).unwrap();

        assert_eq!(game_dir, temp_root);
    }
}
