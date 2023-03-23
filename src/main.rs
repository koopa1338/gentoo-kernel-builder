use config::{Config, Environment, File};
use gentoo_kernel_builder::{BuilderErr, GKBConfig, KernelBuilder};
use std::path::PathBuf;
use sudo;

fn main() -> Result<(), BuilderErr> {
    let mut settings_path = PathBuf::from(std::env!("HOME"));
    settings_path.push(".config/gkb/config");
    let settings = Config::builder()
        .add_source(File::with_name(settings_path.to_string_lossy().as_ref()).required(true))
        .add_source(Environment::with_prefix("GKB"))
        .build()?;

    let config = settings.try_deserialize::<GKBConfig>()?;
    let kernel_builder = KernelBuilder::new(config);

    sudo::escalate_if_needed().map_err(|_| BuilderErr::NoPrivileges)?;
    kernel_builder.build()?;

    Ok(())
}
