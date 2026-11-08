mod apd;
mod assets;
mod auth_list;
mod bootstate;
mod cli;
mod daemon;
mod defs;
mod deploy;
mod event;
mod insmod;
mod late_load;
mod lua;
mod magica;
mod metamodule;
mod module;
mod module_config;
mod package;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod pty;
mod resetprop;
mod restorecon;
mod sepolicy;
mod supercall;
mod utils;
fn main() -> anyhow::Result<()> {
    cli::run()
}
