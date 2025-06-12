use crate::utils::{System, Worker};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::info;

/// An experiment to install and configure a Rust-based replacement for a system utility.
pub struct UutilsExperiment {
    name: String,
    system: System,
    package: String,
    first_supported_release: String,
    unified_binary: Option<PathBuf>,
    bin_directory: PathBuf,
}

impl UutilsExperiment {
    /// Create a new UutilsExperiment.
    pub fn new(
        name: &str,
        system: &System,
        package: &str,
        first_supported_release: &str,
        unified_binary: Option<PathBuf>,
        bin_directory: PathBuf,
    ) -> Self {
        Self {
            name: name.to_string(),
            system: system.clone(),
            package: package.to_string(),
            first_supported_release: first_supported_release.to_string(),
            unified_binary,
            bin_directory,
        }
    }

    /// Check if the system is compatible with the experiment.
    pub fn check_compatible(&self) -> bool {
        self.system
            .distribution()
            .expect("unable to determine distribution information")
            .release
            >= self.first_supported_release
    }

    /// Reports the first supported release for the experiment.
    pub fn first_supported_release(&self) -> &str {
        &self.first_supported_release
    }

    /// Check if the package is installed.
    pub fn check_installed(&self) -> bool {
        self.system.check_installed(&self.package).unwrap_or(false)
    }

    /// Report the name of the experiment.
    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// Enable the experiment by installing and configuring the package.
    pub fn enable(&self) -> Result<()> {
        info!("Installing and configuring {}", self.package);
        self.system.install_package(&self.package)?;

        let files = self.system.list_files(self.bin_directory.clone())?;

        for f in files {
            let filename = f.file_name().unwrap().to_str().unwrap();
            let existing = match self.system.which(filename) {
                Ok(path) => path,
                Err(_) => Path::new("/usr/bin").join(filename),
            };

            if let Some(unified_binary) = &self.unified_binary {
                self.system
                    .replace_file_with_symlink(unified_binary.to_path_buf(), existing.clone())?;
            } else {
                self.system.replace_file_with_symlink(f, existing)?;
            }
        }

        Ok(())
    }

    /// Disable the experiment by removing the package and restoring the original files.
    pub fn disable(&self) -> Result<()> {
        let files = self.system.list_files(self.bin_directory.clone())?;

        for f in files {
            let filename = f.file_name().unwrap().to_str().unwrap();
            let existing = match self.system.which(filename) {
                Ok(path) => path,
                Err(_) => Path::new("/usr/bin").join(filename),
            };
            self.system.restore_file(existing)?;
        }

        info!("Removing {}", self.package);
        self.system.remove_package(&self.package)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{vecs_eq, Distribution, MockSystem};

    #[test]
    fn test_uutils_incompatible_distribution() {
        let runner = incompatible_runner();
        let coreutils = coreutils_fixture(&runner);
        assert!(!coreutils.check_compatible());
    }

    #[test]
    fn test_uutils_install_success_unified_binary() {
        let runner = coreutils_compatible_runner();
        let coreutils = coreutils_fixture(&runner);

        assert!(coreutils.enable().is_ok());

        let commands = runner.commands.clone().into_inner();
        assert_eq!(commands, &["apt-get install -y rust-coreutils"]);

        let backed_up_files = runner.backed_up_files.clone().into_inner();
        let expected = vec!["/usr/bin/date".to_string(), "/usr/bin/sort".to_string()];
        assert!(vecs_eq(backed_up_files, expected));

        let created_symlinks = runner.created_symlinks.clone().into_inner();
        let expected = vec![
            (
                "/usr/bin/coreutils".to_string(),
                "/usr/bin/sort".to_string(),
            ),
            (
                "/usr/bin/coreutils".to_string(),
                "/usr/bin/date".to_string(),
            ),
        ];

        assert!(vecs_eq(created_symlinks, expected));
        assert_eq!(runner.restored_files.clone().into_inner().len(), 0);
    }

    #[test]
    fn test_uutils_install_success_non_unified_binary() {
        let runner = findutils_compatible_runner();
        let findutils = findutils_fixture(&runner);

        assert!(findutils.enable().is_ok());

        let commands = runner.commands.clone().into_inner();
        assert_eq!(commands, &["apt-get install -y rust-findutils"]);

        let backed_up_files = runner.backed_up_files.clone().into_inner();
        let expected = vec!["/usr/bin/find".to_string(), "/usr/bin/xargs".to_string()];
        assert!(vecs_eq(backed_up_files, expected));

        let created_symlinks = runner.created_symlinks.clone().into_inner();
        let expected = vec![
            (
                "/usr/lib/cargo/bin/findutils/find".to_string(),
                "/usr/bin/find".to_string(),
            ),
            (
                "/usr/lib/cargo/bin/findutils/xargs".to_string(),
                "/usr/bin/xargs".to_string(),
            ),
        ];

        assert!(vecs_eq(created_symlinks, expected));
        assert_eq!(runner.restored_files.clone().into_inner().len(), 0);
    }

    #[test]
    fn test_uutils_restore_installed() {
        let runner = coreutils_compatible_runner();
        runner.mock_install_package("rust-coreutils");

        let coreutils = coreutils_fixture(&runner);
        assert!(coreutils.disable().is_ok());

        assert_eq!(runner.created_symlinks.clone().into_inner().len(), 0);
        assert_eq!(runner.backed_up_files.clone().into_inner().len(), 0);

        let commands = runner.commands.clone().into_inner();
        assert_eq!(commands.len(), 1);
        assert!(commands.contains(&"apt-get remove -y rust-coreutils".to_string()));

        let restored_files = runner.restored_files.clone().into_inner();
        let expected = vec!["/usr/bin/date".to_string(), "/usr/bin/sort".to_string()];
        assert!(vecs_eq(restored_files, expected));
    }

    fn coreutils_fixture(system: &MockSystem) -> UutilsExperiment {
        UutilsExperiment::new(
            "coreutils",
            system,
            "rust-coreutils",
            "24.04",
            Some(PathBuf::from("/usr/bin/coreutils")),
            PathBuf::from("/usr/lib/cargo/bin/coreutils"),
        )
    }

    fn coreutils_compatible_runner() -> MockSystem {
        let runner = MockSystem::default();
        runner.mock_files(vec![
            ("/usr/lib/cargo/bin/coreutils/date", "", false),
            ("/usr/lib/cargo/bin/coreutils/sort", "", false),
            ("/usr/bin/sort", "", true),
            ("/usr/bin/date", "", true),
        ]);
        runner
    }

    fn findutils_fixture(system: &MockSystem) -> UutilsExperiment {
        UutilsExperiment::new(
            "findutils",
            system,
            "rust-findutils",
            "24.04",
            None,
            PathBuf::from("/usr/lib/cargo/bin/findutils"),
        )
    }

    fn findutils_compatible_runner() -> MockSystem {
        let runner = MockSystem::default();
        runner.mock_files(vec![
            ("/usr/lib/cargo/bin/findutils/find", "", false),
            ("/usr/lib/cargo/bin/findutils/xargs", "", false),
            ("/usr/bin/find", "", true),
            ("/usr/bin/xargs", "", true),
        ]);
        runner
    }

    fn incompatible_runner() -> MockSystem {
        MockSystem::new(Distribution {
            id: "Ubuntu".to_string(),
            release: "20.04".to_string(),
        })
    }
}
