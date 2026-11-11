//! Everything that differs between operating systems.
//!
//! This is the only module that uses `cfg(...)`. The rest of the core asks
//! questions here ("where is the config folder?", "make this file private")
//! and never checks the operating system itself.
//!
//! Most functions take their inputs (environment, home folder, folders to
//! search) as arguments instead of reading them from the process. That keeps
//! them testable: a test passes made-up values and gets the same answer on
//! every machine.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
    Other,
}

pub fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Other
    }
}

// --- Folders ---------------------------------------------------------------

/// A way to read environment variables. The real one is `&|k| std::env::var_os(k)`.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// Where Slipdock keeps profiles and settings. First match wins:
/// 1. `explicit` (the `--config-dir` option)
/// 2. `$SLIPDOCK_CONFIG_DIR`
/// 3. `$XDG_CONFIG_HOME/slipdock`
/// 4. the usual place for this operating system
pub fn config_dir(
    explicit: Option<&Path>,
    env: EnvLookup,
    os: Os,
    home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return Ok(dir.to_path_buf());
    }
    // An empty variable counts as unset, as the XDG specification says.
    let var = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(dir) = var("SLIPDOCK_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("slipdock"));
    }
    let home = home.ok_or_else(|| {
        Error::msg("cannot find your home folder; set SLIPDOCK_CONFIG_DIR or pass --config-dir")
    })?;
    Ok(match os {
        Os::MacOs => home.join("Library/Application Support/slipdock"),
        Os::Windows => home.join("AppData/Roaming/slipdock"),
        Os::Linux | Os::Other => home.join(".config/slipdock"),
    })
}

/// Replaces a leading `~` with the home folder: `~/dumps` -> `/home/me/dumps`.
/// Only a bare `~` or `~/...` is expanded, not `~someone`.
pub fn expand_tilde(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

// --- Private files ---------------------------------------------------------

/// Creates a folder (and its parents) that only the owner can enter: mode 0700.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)
}

/// Writes a file that only the owner can read: mode 0600.
/// An existing file is replaced and its mode tightened.
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = private_open_options().truncate(true).open(path)?;
    file.write_all(contents)?;
    set_mode(path, 0o600)
}

/// Like [`write_private_file`], but fails when the file already exists.
pub fn create_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = private_open_options().create_new(true).open(path)?;
    file.write_all(contents)
}

fn private_open_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        // The mode is given at creation, so the file is never readable by
        // others, not even for a moment.
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    // Windows has no owner/group/other bits; files in the user's profile
    // folder are private by default.
    Ok(())
}

// --- Processes -------------------------------------------------------------

/// Asks a running process to stop (SIGTERM). mydumper and myloader finish the
/// current step and exit; nothing is killed by force.
#[cfg(unix)]
pub fn terminate(pid: u32) -> io::Result<()> {
    use rustix::process::{Pid, Signal, kill_process};
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process id"))?;
    kill_process(pid, Signal::TERM).map_err(io::Error::from)
}

#[cfg(not(unix))]
pub fn terminate(_pid: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stopping a tool is not supported on this system",
    ))
}

/// True for a regular file that may be run.
pub fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// --- Where tools live ------------------------------------------------------

/// The folders in `$PATH`, in order.
pub fn path_dirs(env: EnvLookup) -> Vec<PathBuf> {
    match env("PATH") {
        Some(path) => std::env::split_paths(&path).collect(),
        None => Vec::new(),
    }
}

/// Folders where package managers put tools, also when they are not on PATH.
/// Homebrew's `mysql-client` is "keg-only": it is installed but not linked
/// into `bin`, so its `opt/mysql-client/bin` folder is listed too.
pub fn usual_tool_dirs(os: Os) -> Vec<PathBuf> {
    let dirs: &[&str] = match os {
        Os::MacOs => &[
            "/opt/homebrew/bin",
            "/opt/homebrew/opt/mysql-client/bin",
            "/usr/local/bin",
            "/usr/local/opt/mysql-client/bin",
            "/opt/local/bin", // MacPorts
            "/usr/bin",
        ],
        Os::Linux | Os::Other => &[
            "/usr/local/bin",
            "/usr/bin",
            "/home/linuxbrew/.linuxbrew/bin",
            "/home/linuxbrew/.linuxbrew/opt/mysql-client/bin",
        ],
        Os::Windows => &[],
    };
    dirs.iter().map(PathBuf::from).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Homebrew,
    Apt,
    Dnf,
    Pacman,
}

impl PackageManager {
    pub const ALL: [PackageManager; 4] = [Self::Homebrew, Self::Apt, Self::Dnf, Self::Pacman];

    /// The program that shows this package manager is present.
    pub fn program(self) -> &'static str {
        match self {
            Self::Homebrew => "brew",
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
        }
    }
}

/// The package managers whose program is found in one of `dirs`.
pub fn package_managers(dirs: &[PathBuf]) -> Vec<PackageManager> {
    PackageManager::ALL
        .into_iter()
        .filter(|manager| {
            dirs.iter()
                .any(|dir| is_executable(&dir.join(manager.program())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn config_dir_prefers_explicit_then_env_then_xdg_then_platform() {
        let home = Path::new("/home/example");
        let all = env_of(&[
            ("SLIPDOCK_CONFIG_DIR", "/from-env"),
            ("XDG_CONFIG_HOME", "/xdg"),
        ]);
        let dir = |explicit, env: EnvLookup, os| config_dir(explicit, env, os, Some(home)).unwrap();

        assert_eq!(
            dir(Some(Path::new("/flag")), &all, Os::Linux),
            Path::new("/flag")
        );
        assert_eq!(dir(None, &all, Os::Linux), Path::new("/from-env"));
        let xdg = env_of(&[("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(dir(None, &xdg, Os::MacOs), Path::new("/xdg/slipdock"));
        let none = env_of(&[]);
        assert_eq!(
            dir(None, &none, Os::Linux),
            Path::new("/home/example/.config/slipdock")
        );
        assert_eq!(
            dir(None, &none, Os::MacOs),
            Path::new("/home/example/Library/Application Support/slipdock")
        );
    }

    #[test]
    fn config_dir_treats_empty_variables_as_unset() {
        let env = env_of(&[("SLIPDOCK_CONFIG_DIR", ""), ("XDG_CONFIG_HOME", "")]);
        let dir = config_dir(None, &env, Os::Linux, Some(Path::new("/h"))).unwrap();
        assert_eq!(dir, Path::new("/h/.config/slipdock"));
    }

    #[test]
    fn config_dir_without_home_explains_the_fix() {
        let error = config_dir(None, &env_of(&[]), Os::Linux, None).unwrap_err();
        assert!(error.to_string().contains("SLIPDOCK_CONFIG_DIR"));
    }

    #[test]
    fn tilde_is_expanded_only_at_the_start() {
        let home = Some(Path::new("/home/example"));
        assert_eq!(expand_tilde("~", home), Path::new("/home/example"));
        assert_eq!(
            expand_tilde("~/dumps", home),
            Path::new("/home/example/dumps")
        );
        assert_eq!(expand_tilde("/a/~/b", home), Path::new("/a/~/b"));
        assert_eq!(expand_tilde("~other/x", home), Path::new("~other/x"));
        assert_eq!(expand_tilde("~/dumps", None), Path::new("~/dumps"));
    }

    #[cfg(unix)]
    #[test]
    fn private_files_and_folders_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let temp = tempfile::tempdir().unwrap();

        let dir = temp.path().join("a/b");
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);

        let file = dir.join("secret.cnf");
        create_private_file(&file, b"one").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert!(create_private_file(&file, b"two").is_err(), "must not overwrite");

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&file, b"three").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(fs::read(&file).unwrap(), b"three");
    }

    #[cfg(unix)]
    #[test]
    fn package_managers_are_found_by_their_program() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let brew = temp.path().join("brew");
        fs::write(&brew, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
        // Present but not executable: does not count.
        fs::write(temp.path().join("dnf"), "").unwrap();

        let found = package_managers(&[temp.path().to_path_buf()]);
        assert_eq!(found, vec![PackageManager::Homebrew]);
    }

    #[cfg(unix)]
    #[test]
    fn terminate_stops_a_running_process() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        terminate(child.id()).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
