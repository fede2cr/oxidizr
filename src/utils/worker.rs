use std::{
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::Result;
use std::fs;
use tracing::{debug, trace, warn};
use which::which;

use super::{Command, Distribution};

pub trait Worker {
    fn distribution(&self) -> Result<Distribution>;
    fn run(&self, cmd: &Command) -> Result<Output>;
    fn list_files(&self, directory: PathBuf) -> Result<Vec<PathBuf>>;
    fn which(&self, binary_name: &str) -> Result<PathBuf>;
    fn install_package(&self, package: &str) -> Result<()>;
    fn remove_package(&self, package: &str) -> Result<()>;
    fn update_package_lists(&self) -> Result<()>;
    fn check_installed(&self, package: &str) -> Result<bool>;
    fn replace_file_with_symlink(&self, source: PathBuf, target: PathBuf) -> Result<()>;
    fn backup_file(&self, file: PathBuf) -> Result<()>;
    fn restore_file(&self, file: PathBuf) -> Result<()>;
    fn create_symlink(&self, source: PathBuf, target: PathBuf) -> Result<()>;
}
/// A struct representing the system with functions for running commands and manipulating
/// files on the filesystem.
#[derive(Clone, Debug)]
pub struct System {
    package_manager: PackageManager,
}

impl System {
    /// Create a new `System` instance.
    pub fn new() -> Result<Self> {
        let dist = get_distribution()?;
        let package_manager = PackageManager::from_distribution(&dist);
        Ok(Self { package_manager })
    }
}

impl Worker for System {
    fn distribution(&self) -> Result<Distribution> {
        get_distribution()
    }

    /// Run a command and return the output. If the command fails, an error will be returned.
    fn run(&self, cmd: &Command) -> Result<Output> {
        debug!("Running command: {}", cmd.command());
        let output = std::process::Command::new(&cmd.command)
            .args(&cmd.args)
            .output()?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to run command '{}': {}",
                &cmd.command(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output)
    }

    /// List files in a directory. If the directory does not exist or is not a directory, an error
    /// will be returned.
    fn list_files(&self, directory: PathBuf) -> Result<Vec<PathBuf>> {
        if !fs::exists(&directory)? || !fs::metadata(&directory)?.is_dir() {
            anyhow::bail!("{} is not a directory", directory.to_str().unwrap());
        }

        let entries = fs::read_dir(directory)?;

        let files = entries
            .map(|entry| {
                let entry = entry?;
                let path = entry.path();
                Ok(path)
            })
            .collect::<Result<Vec<PathBuf>>>()?;

        Ok(files)
    }

    /// Find the path to a binary in the system's PATH.
    fn which(&self, binary_name: &str) -> Result<PathBuf> {
        Ok(which(binary_name)?)
    }

    /// Install a package using the system package manager.
    fn install_package(&self, package: &str) -> Result<()> {
        let cmd = match self.package_manager {
            PackageManager::Apt => Command::build("apt-get", &["install", "-y", package]),
            PackageManager::Tdnf => Command::build("tdnf", &["install", "-y", package]),
            PackageManager::Dnf => Command::build("dnf", &["install", "-y", package]),
            PackageManager::Unknown => anyhow::bail!("Unknown package manager"),
        };
        self.run(&cmd)?;
        Ok(())
    }

    /// Remove a package using the system package manager.
    fn remove_package(&self, package: &str) -> Result<()> {
        let cmd = match self.package_manager {
            PackageManager::Apt => Command::build("apt-get", &["remove", "-y", package]),
            PackageManager::Tdnf => Command::build("tdnf", &["remove", "-y", package]),
            PackageManager::Dnf => Command::build("dnf", &["remove", "-y", package]),
            PackageManager::Unknown => anyhow::bail!("Unknown package manager"),
        };
        self.run(&cmd)?;
        Ok(())
    }

    /// Update the package lists using the system package manager.
    fn update_package_lists(&self) -> Result<()> {
        let cmd = match self.package_manager {
            PackageManager::Apt => Command::build("apt-get", &["update"]),
            PackageManager::Tdnf => Command::build("tdnf", &["makecache"]),
            PackageManager::Dnf => Command::build("dnf", &["makecache"]),
            PackageManager::Unknown => anyhow::bail!("Unknown package manager"),
        };
        self.run(&cmd)?;
        Ok(())
    }

    /// Check if a package is installed using the system package manager.
    fn check_installed(&self, package: &str) -> Result<bool> {
        let cmd = match self.package_manager {
            PackageManager::Apt => Command::build("dpkg-query", &["-s", package]),
            PackageManager::Tdnf => Command::build("rpm", &["-q", package]),
            PackageManager::Dnf => Command::build("rpm", &["-q", package]),
            PackageManager::Unknown => anyhow::bail!("Unknown package manager"),
        };
        match self.run(&cmd) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Replace a file with a symlink. If the target file already exists, it will be backed up.
    fn replace_file_with_symlink(&self, source: PathBuf, target: PathBuf) -> Result<()> {
        if fs::exists(&target)? {
            if target.is_symlink() {
                trace!("Skipping {}, symlink already exists", target.display());
                return Ok(());
            }
            self.backup_file(target.clone())?;
            fs::remove_file(&target)?;
        }

        self.create_symlink(source, target)?;
        Ok(())
    }

    /// Backup a file by copying it to a new file with a `.oxidizr.bak` extension.
    fn backup_file(&self, file: PathBuf) -> Result<()> {
        let backup_file = backup_filename(&file);
        trace!("Backing up {} -> {}", file.display(), backup_file.display());
        fs::copy(&file, &backup_file)?;

        // Ensure the same permissions are set on the backup file as on the original file.
        // This accounts for permissions such as SUID, SGID, and sticky bits which are not
        // preserved by `fs::copy`.
        let metadata = fs::metadata(&file)?;
        fs::set_permissions(&backup_file, metadata.permissions())?;
        Ok(())
    }

    /// Restore a file from a backup if the backup file exists, warn otherwise.
    fn restore_file(&self, file: PathBuf) -> Result<()> {
        let backup_file = backup_filename(&file);

        if fs::exists(&backup_file)? {
            trace!("Restoring {} -> {}", backup_file.display(), file.display());
            fs::rename(&backup_file, &file)?;
        } else {
            warn!("No backup found for '{}', skipping restore", file.display());
        }

        Ok(())
    }

    /// Create a symlink from `source` to `target`. If `target` already exists, it will be removed.
    fn create_symlink(&self, source: PathBuf, target: PathBuf) -> Result<()> {
        trace!("Symlinking {} -> {}", source.display(), target.display());
        remove_file_if_exists(&target)?;
        std::os::unix::fs::symlink(source, target)?;
        Ok(())
    }
}

/// Generate a backup filename. For a given file `/path/to/file`, the backup filename will be
/// `/path/to/.file.oxidizr.bak`.
fn backup_filename(file: &Path) -> PathBuf {
    let mut backup_file = file.parent().unwrap_or(&PathBuf::from(".")).to_path_buf();
    backup_file.push(format!(
        ".{}.oxidizr.bak",
        file.file_name().unwrap().to_string_lossy()
    ));
    backup_file
}

/// Remove a file from the filesystem if it exists.
fn remove_file_if_exists(file: &PathBuf) -> Result<()> {
    if fs::exists(file)? {
        fs::remove_file(file)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::utils::worker::backup_filename;

    #[test]
    fn test_backup_filename() {
        let file = PathBuf::from("/home/user/config");
        let backup = backup_filename(&file);
        assert_eq!(backup, PathBuf::from("/home/user/.config.oxidizr.bak"));

        let file = PathBuf::from("config");
        let backup = backup_filename(&file);
        assert_eq!(backup, PathBuf::from(".config.oxidizr.bak"));

        let file = PathBuf::from("/etc/hosts");
        let backup = backup_filename(&file);
        assert_eq!(backup, PathBuf::from("/etc/.hosts.oxidizr.bak"));

        let file = PathBuf::from(".hidden");
        let backup = backup_filename(&file);
        assert_eq!(backup, PathBuf::from("..hidden.oxidizr.bak"));
    }
}

#[derive(Clone, Debug)]
pub enum PackageManager {
    Apt,
    Tdnf,
    Dnf,
    Unknown,
}

impl PackageManager {
    pub fn from_distribution(dist: &Distribution) -> Self {
        match dist.id.to_lowercase().as_str() {
            "ubuntu" | "debian" => PackageManager::Apt,
            "azurelinux" => PackageManager::Tdnf,
            "fedora" => PackageManager::Dnf,
            _ => PackageManager::Unknown,
        }
    }
}

// Add this helper function near the top or bottom of your file:
fn get_distribution() -> Result<Distribution> {
    let id_output = std::process::Command::new("lsb_release")
        .arg("-is")
        .output()?;
    let release_output = std::process::Command::new("lsb_release")
        .arg("-rs")
        .output()?;

    Ok(Distribution {
        id: String::from_utf8(id_output.stdout)?.trim().to_string(),
        release: String::from_utf8(release_output.stdout)?.trim().to_string(),
    })
}
