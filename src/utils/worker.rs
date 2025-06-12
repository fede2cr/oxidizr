use std::{
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::Result;
use sys_info::LinuxOSReleaseInfo;
use std::fs;
use tracing::{debug, trace, warn, info};
use which::which;

use super::{Command, Distribution};

fn has_docker_in_cgroup() -> bool {
    match fs::read_to_string("/proc/self/cgroup") {
        Ok(file_contents) => file_contents.contains("docker"),
        Err(_error) => false,
    }
}

// /// Supported/known Linux distributions
#[derive(Clone, Debug, PartialEq)]
pub enum SupportedLinuxDistribution {
    Ubuntu,
    AzureLinux,
    Fedora,
    ArchLinux,
}

impl TryFrom<LinuxOSReleaseInfo> for SupportedLinuxDistribution {
    type Error = anyhow::Error;

    fn try_from(release_info: LinuxOSReleaseInfo) -> Result<Self, Self::Error> {
        match release_info.id() {
            "ubuntu" => Ok(SupportedLinuxDistribution::Ubuntu),
            "azurelinux" => Ok(SupportedLinuxDistribution::AzureLinux),
            "fedora" => Ok(SupportedLinuxDistribution::Fedora),
            "arch" => Ok(SupportedLinuxDistribution::ArchLinux),
            _ => Err(anyhow::anyhow!("Unsupported Linux distribution: {}", release_info.id())),
        }
    }
}

impl SupportedLinuxDistribution {
    pub fn check_compatibility_status(&self, no_compatibility_check: bool) {
        if SupportedLinuxDistribution::Ubuntu == *self {
            info!("Ubuntu is supported.");
            return;
        }
        if no_compatibility_check {
            let is_docker = has_docker_in_cgroup();
            if is_docker {
                info!("/proc/self/cgroup contains docker, running without compatibility checks enabled");
            } else {
                warn!("/proc/self/cgroup does not contain docker and running without compatibility checks enabled, this may cause system instability.");
            }
            match self {
                SupportedLinuxDistribution::AzureLinux => info!("Azure Linux support is experimental."),
                SupportedLinuxDistribution::Fedora => info!("Fedora support is experimental."),
                SupportedLinuxDistribution::ArchLinux => info!("Arch Linux support is experimental."),
                _ => unreachable!(),
            }
        }
    }
}

impl Worker for SupportedLinuxDistribution {
    /// Generates a commmand to check if a package is installed.
    fn gen_check_installed_command(&self, package: &str) -> Command {
        match self {
            SupportedLinuxDistribution::Ubuntu | SupportedLinuxDistribution::AzureLinux => {
                Command::build("dpkg-query", &["-s", package])
            }
            SupportedLinuxDistribution::Fedora => {
                Command::build("rpm", &["-q", package])
            }
            SupportedLinuxDistribution::ArchLinux => {
                Command::build("pacman", &["-Qi", package])
            }
        }
    }
}

pub trait Worker {
    /// Each distributon must implement a way to check if a package is installed.
    fn gen_check_installed_command(&self, package: &str) -> Command;

    /// Report the distribution information for the system.
    fn distribution(&self) -> Result<Distribution> {
        let cmd = Command::build("lsb_release", &["-is"]);
        let id = self.run(&cmd)?;

        let cmd = Command::build("lsb_release", &["-rs"]);
        let release = self.run(&cmd)?;

        Ok(Distribution {
            id: String::from_utf8(id.stdout)?.trim().to_string(),
            release: String::from_utf8(release.stdout)?.trim().to_string(),
        })
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
        let cmd = Command::build("apt-get", &["install", "-y", package]);
        self.run(&cmd)?;
        Ok(())
    }

    /// Remove a package using the system package manager.
    fn remove_package(&self, package: &str) -> Result<()> {
        let cmd = Command::build("apt-get", &["remove", "-y", package]);
        self.run(&cmd)?;
        Ok(())
    }

    /// Update the package lists using the system package manager.
    fn update_package_lists(&self) -> Result<()> {
        let cmd = Command::build("apt-get", &["update"]);
        self.run(&cmd)?;
        Ok(())
    }

    /// Check if a package is installed using the system package manager.
    fn check_installed(&self, package: &str) -> Result<bool> {
        let cmd = self.gen_check_installed_command(package);
        match self.run(&cmd) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Replace a file with a symlink. If the target file already exists, it will be backed up
    /// before being replaced.
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

    /// Backup a file by copying it to a new file with the same name, but with a `.oxidizr.bak`
    /// extension.
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

    /// Restore a file from a backup. If the backup file does not exist, the original file will be
    /// left untouched.
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

    /// Create a symlink from `source` to `target`. If `target` already exists, it will be
    /// removed and overwritten with the symlink.
    fn create_symlink(&self, source: PathBuf, target: PathBuf) -> Result<()> {
        trace!("Symlinking {} -> {}", source.display(), target.display());
        remove_file_if_exists(&target)?;
        std::os::unix::fs::symlink(source, target)?;
        Ok(())
    }
}

/// A struct representing the system with functions for running commands and manipulating
/// files on the filesystem.
#[derive(Copy, Clone, Debug)]
pub struct System {
    /// Each linux distribution install packages via different commands.
    linux_distribution: SupportedLinuxDistribution,
}

impl System {
    /// Create a new `System` instance.
    pub fn new(linux_distribution: SupportedLinuxDistribution) -> Result<Self> {
        Ok(Self {
            linux_distribution,
        })
    }
}

impl Worker for System {

    fn gen_check_installed_command(&self, package: &str) -> Command {
        self.linux_distribution.gen_check_installed_command(package)
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
