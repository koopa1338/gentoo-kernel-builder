use dialoguer::theme::ColorfulTheme;
use dialoguer::Select;
use dialoguer::{console::Term, Confirm};
use indicatif::ProgressBar;
use serde::Deserialize;
use std::num::NonZeroUsize;
use std::{
    os::unix,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

mod error;
pub use error::BuilderErr;

#[derive(Debug, Deserialize)]
pub struct GKBConfig {
    /// Path to the kernel bz image on the boot partition
    #[serde(rename = "kernel")]
    pub kernel_file_path: PathBuf,
    /// Path to the initramfs on the boot partition
    #[serde(rename = "initramfs")]
    pub initramfs_file_path: PathBuf,
    /// path to the `.config` file that will be symlinked
    #[serde(rename = "kernel-config")]
    pub kernel_config_file_path: PathBuf,
}

#[derive(Clone, Debug)]
struct VersionEntry {
    path: PathBuf,
    version_string: String,
}

#[derive(Debug)]
pub struct KernelBuilder {
    config: GKBConfig,
    versions: Vec<VersionEntry>,
}

impl KernelBuilder {
    pub const LINUX_PATH: &str = "/usr/src";

    #[must_use]
    pub fn new(config: GKBConfig) -> Self {
        let mut builder = Self {
            config,
            versions: vec![],
        };
        builder.get_available_version();

        builder
    }

    fn get_available_version(&mut self) {
        if self.versions.is_empty() {
            if let Ok(directories) = std::fs::read_dir(Self::LINUX_PATH) {
                self.versions = directories
                    .filter_map(Result::ok)
                    .map(|dir| dir.path())
                    .filter(|path| path.starts_with(Self::LINUX_PATH) && !path.is_symlink())
                    .filter_map(|path| {
                        path.strip_prefix(Self::LINUX_PATH).ok().and_then(|p| {
                            let tmp = p.to_owned();
                            let version_string = tmp.to_string_lossy();
                            (version_string.starts_with("linux")
                                && version_string.ends_with("gentoo"))
                            .then_some(VersionEntry {
                                path: path.clone(),
                                version_string: version_string.to_string(),
                            })
                        })
                    })
                    .collect::<Vec<_>>();
            }
        }
    }

    pub fn build(&self) -> Result<(), BuilderErr> {
        let version_entry = self.prompt_for_kernel_version();
        let VersionEntry {
            path,
            version_string,
        } = &version_entry;

        // create symlink from /usr/src/.config
        let link = path.join(".config");
        if !link.exists() {
            let dot_config = &self.config.kernel_config_file_path;
            if !dot_config.exists() || !dot_config.is_file() {
                return Err(BuilderErr::KernelConfigMissing);
            }

            unix::fs::symlink(dot_config, link).map_err(|err| BuilderErr::LinkingFileError(err))?;
        }

        let linux = PathBuf::from(Self::LINUX_PATH).join("linux");
        let linux_target = linux
            .read_link()
            .map_err(|err| BuilderErr::LinkingFileError(err))?;

        if linux_target.to_string_lossy() != *version_string {
            std::fs::remove_file(&linux).map_err(|err| BuilderErr::LinkingFileError(err))?;
            unix::fs::symlink(path, linux).map_err(|err| BuilderErr::LinkingFileError(err))?;
        }

        self.build_kernel(path)?;

        if self.confirm_prompt("Do you want to install kernel modules?")? {
            self.install_kernel_modules(path)?;
        }

        if self.confirm_prompt("Do you want to generate initramfs with dracut?")? {
            self.generate_initramfs(&version_entry)?;
        }

        Ok(())
    }

    fn build_kernel(&self, path: &Path) -> Result<(), BuilderErr> {
        let threads: NonZeroUsize =
            std::thread::available_parallelism().unwrap_or(NonZeroUsize::new(1).unwrap());
        let pb = ProgressBar::new_spinner();
        pb.enable_steady_tick(Duration::from_millis(120));
        pb.set_message("Compiling kernel...");
        Command::new("make")
            .current_dir(path)
            .args(["-j", &threads.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| BuilderErr::KernelBuildFail(err))?
            .wait()
            .map_err(|err| BuilderErr::KernelBuildFail(err))?;
        pb.finish_with_message("Finished compiling Kernel");
        std::fs::copy(
            path.join("arch/x86/boot/bzImage"),
            self.config.kernel_file_path.clone(),
        )
        .map_err(|err| BuilderErr::KernelBuildFail(err))?;

        Ok(())
    }

    fn install_kernel_modules(&self, path: &Path) -> Result<(), BuilderErr> {
        let pb = ProgressBar::new_spinner();
        pb.enable_steady_tick(Duration::from_millis(120));
        pb.set_message("Install kernel modules");
        Command::new("make")
            .current_dir(path)
            .arg("modules_install")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                BuilderErr::KernelBuildFail(err)
            })?
            .wait()
            .map_err(|err| {
                BuilderErr::KernelBuildFail(err)
            })?;
        pb.finish_with_message("Finished installing modules");

        Ok(())
    }

    fn generate_initramfs(
        &self,
        VersionEntry {
            path,
            version_string,
        }: &VersionEntry,
    ) -> Result<(), BuilderErr> {
        let pb = ProgressBar::new_spinner();
        pb.enable_steady_tick(Duration::from_millis(120));
        pb.set_message("Gen initramfs");
        Command::new("dracut")
            .current_dir(path)
            .args([
                "--hostonly",
                "--kver",
                version_string.strip_prefix("linux-").unwrap(),
                "--force",
                self.config.initramfs_file_path.to_string_lossy().as_ref(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                BuilderErr::KernelBuildFail(err)
            })?
            .wait()
            .map_err(|err| {
                BuilderErr::KernelBuildFail(err)
            })?;
        pb.finish_with_message("Finished initramfs");

        Ok(())
    }

    fn prompt_for_kernel_version(&self) -> VersionEntry {
        let versions = self
            .versions
            .clone()
            .into_iter()
            .map(|v| v.version_string)
            .collect::<Vec<_>>();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Pick version to build and install")
            .items(versions.as_slice())
            .default(0)
            .interact_on_opt(&Term::stderr())
            .unwrap()
            .unwrap();
        self.versions[selection].clone()
    }

    fn confirm_prompt(&self, message: &str) -> Result<bool, BuilderErr> {
        Confirm::new()
            .with_prompt(message)
            .interact()
            .map_err(|err| BuilderErr::PromptError(err))
    }
}
