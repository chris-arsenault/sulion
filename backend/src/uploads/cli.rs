use anyhow::{ensure, Context};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use super::{install::Directory, model::valid_component, MAX_BYTES};

pub fn run(args: &[OsString]) -> anyhow::Result<i32> {
    if args.is_empty() || matches!(args[0].to_str(), Some("help" | "--help" | "-h")) {
        println!("Usage: sulion upload [-C CHECKOUT] SOURCE DESTINATION\nInstall a local file at a checkout-relative path, replacing the destination atomically (up to 50 MiB).");
        return Ok(if args.is_empty() { 64 } else { 0 });
    }
    let (root, paths) = if let [flag, directory, rest @ ..] = args {
        if flag == "-C" {
            (PathBuf::from(directory), rest)
        } else {
            (current_checkout()?, args)
        }
    } else {
        anyhow::bail!("supply a source file and checkout-relative destination")
    };
    let [source, destination] = paths else {
        anyhow::bail!("usage: sulion upload [-C CHECKOUT] SOURCE DESTINATION")
    };
    let destination = destination.to_str().context("destination must be UTF-8")?;
    install(&root, source, destination)?;
    println!("{destination}");
    Ok(0)
}

fn current_checkout() -> anyhow::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let git = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if git.status.success() {
        return Ok(OsStr::from_bytes(git.stdout.strip_suffix(b"\n").unwrap_or(&git.stdout)).into());
    }
    Ok(std::env::current_dir()?)
}

fn install(root: &Path, source: &OsStr, destination: &str) -> anyhow::Result<()> {
    let (parent, filename) = destination.rsplit_once('/').unwrap_or(("", destination));
    ensure!(
        destination.split('/').all(valid_component)
            && parent.len() <= 1024
            && filename.len() <= 255,
        "invalid checkout-relative destination"
    );
    let input = File::open(source)?;
    let meta = input.metadata()?;
    ensure!(meta.is_file(), "expected a regular source file");
    ensure!(meta.len() <= MAX_BYTES as u64, "file exceeds 50 MiB");
    let mut temporary = Directory::root(root)?.descend(parent, true)?.temporary()?;
    let mut output = temporary.take_file();
    let count = io::copy(&mut input.take(MAX_BYTES as u64 + 1), &mut output)?;
    ensure!(count == meta.len(), "source size changed during transfer");
    output.sync_all()?;
    temporary.install_at(root, parent, filename)?;
    Ok(())
}
