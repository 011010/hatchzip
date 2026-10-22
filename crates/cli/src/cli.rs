//! Command line definition. Also the source of the shell completions and the
//! man page, so the three never disagree.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_NO_SPACE: u8 = 3;
pub const EXIT_DAMAGED: u8 = 4;
pub const EXIT_PASSWORD: u8 = 5;
pub const EXIT_PAUSED: u8 = 130;

const ABOUT: &str = "Extract huge archives in place: the archive shrinks as your files hatch.";

const LONG_ABOUT: &str = "\
Extract huge archives in place: the archive shrinks as your files hatch.

HatchZip extracts an archive from its end and cuts each finished part off the \
archive file as it goes, so the space the archive gives back is reused for the \
next files. You need free space for the output, not for the output and the \
archive. It checks the plan fits before touching anything, and an interrupted \
run continues where it stopped when you run the same command again.

Supported: ZIP (including ZIP64 and self-extracting .exe), 7z, uncompressed tar, \
and split archives (.001, .002, ...) of any of those.";

const AFTER_LONG_HELP: &str = "\
Examples:
  hatchzip check game.zip              Will it fit? Changes nothing.
  hatchzip extract game.zip            Extract into ./game, deleting game.zip as it goes
  hatchzip x game.7z.001 -o D:\\Games   Split archive, any part name works
  hatchzip extract big.zip --verify-first --keep-free 10GB
  hatchzip test game.7z                Check every file for damage, write nothing
  hatchzip restore game.zip            Stopped halfway? Make what is left a normal archive

Interrupted? Run the exact same command again. Progress lives in two files next \
to the archive (<archive>.hatchzip and <archive>.hatchzip-tail); don't delete them \
while an extraction is unfinished. To stop for good instead, run restore: the \
entries not yet extracted become a normal archive again.

Exit status:
  0    success
  1    failure (progress, if any, is saved)
  2    invalid command line
  3    not enough free space; nothing was changed
  4    the archive is damaged; nothing was deleted
  5    the archive is password protected and no right password was given
  130  paused with Ctrl+C; run the same command to continue

Encrypted archives (ZIP with ZipCrypto or AES, 7z with AES): HatchZip asks for the \
password, or takes --password or the HATCHZIP_PASSWORD environment variable. It \
proves the password on one file before anything is deleted. The password is never \
saved, so continuing an interrupted run asks again.";

#[derive(Parser)]
#[command(
    name = "hatchzip",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Print only errors and prompts
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Extract an archive, deleting it as it goes
    #[command(
        visible_alias = "x",
        long_about = "Extract an archive, deleting it as it goes.\n\n\
            Prints the space plan and asks before starting. If interrupted by Ctrl+C, a \
            crash or a full disk, run the same command again to continue."
    )]
    Extract {
        #[command(flatten)]
        target: Target,
        /// Read the whole archive and check every file before deleting anything
        #[arg(long)]
        verify_first: bool,
        #[command(flatten)]
        space: Space,
        /// Don't ask for confirmation (required when not run from a terminal)
        #[arg(short, long)]
        yes: bool,
    },
    /// Show whether an archive fits on this disk, without changing anything
    #[command(visible_alias = "c")]
    Check {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        space: Space,
    },
    /// List what an archive contains
    #[command(visible_alias = "l")]
    List {
        /// The archive, or any part of a split archive
        archive: PathBuf,
        #[command(flatten)]
        secret: Secret,
    },
    /// Check every file in an archive for damage, without writing anything
    #[command(visible_alias = "t")]
    Test {
        /// The archive, or any part of a split archive
        archive: PathBuf,
        #[command(flatten)]
        secret: Secret,
    },
    /// Turn a half-extracted archive back into a normal one other tools open
    #[command(
        long_about = "Turn a half-extracted archive back into a normal one other tools open.\n\n\
            An interrupted extraction leaves the archive holding only the entries not yet \
            extracted, with its index saved next to it, so only HatchZip can open it. restore \
            appends a new index for those entries, so 7-Zip, WinRAR, unzip or tar open it \
            again. Nothing is moved: it needs only the space the new index takes. Files \
            already extracted stay in the output folder. If restore is interrupted, run it \
            again, or run extract to continue extracting instead."
    )]
    Restore {
        /// The half-extracted archive, or any part of a split archive
        archive: PathBuf,
    },
    /// Print a shell completion script
    #[command(long_about = "Print a shell completion script.\n\n\
        bash:        hatchzip completions bash > ~/.local/share/bash-completion/completions/hatchzip\n\
        zsh:         hatchzip completions zsh > \"${fpath[1]}/_hatchzip\"\n\
        fish:        hatchzip completions fish > ~/.config/fish/completions/hatchzip.fish\n\
        PowerShell:  hatchzip completions powershell >> $PROFILE")]
    Completions { shell: Shell },
    /// Print the man page (roff)
    #[command(long_about = "Print the man page in roff format.\n\n\
        hatchzip man > ~/.local/share/man/man1/hatchzip.1")]
    Man,
}

#[derive(Args)]
pub struct Target {
    /// The archive, or any part of a split archive
    pub archive: PathBuf,
    /// Where to put the files [default: a folder named after the archive, next to it]
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,
    #[command(flatten)]
    pub secret: Secret,
}

#[derive(Args)]
pub struct Secret {
    /// Password for an encrypted archive. Without it HatchZip asks, hiding
    /// what you type; prefer that, since this is visible to other programs
    #[arg(
        short,
        long,
        env = "HATCHZIP_PASSWORD",
        hide_env_values = true,
        value_name = "PASSWORD"
    )]
    pub password: Option<String>,
}

#[derive(Args)]
pub struct Space {
    /// Free space HatchZip never uses, so the disk never fills to zero
    #[arg(long, value_name = "SIZE", default_value = "2GB", value_parser = parse_size)]
    pub keep_free: u64,
}

/// Sizes like `500MB`, `2GB` or `1.5TB`, in powers of 1000 to match what
/// HatchZip prints.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num
        .parse()
        .map_err(|_| format!("'{s}' is not a size like 500MB or 2GB"))?;
    let mult = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "K" | "KB" => 1e3,
        "M" | "MB" => 1e6,
        "G" | "GB" => 1e9,
        "T" | "TB" => 1e12,
        _ => return Err(format!("unknown unit in '{s}'; use B, KB, MB, GB or TB")),
    };
    Ok((num * mult) as u64)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("0"), Ok(0));
        assert_eq!(parse_size("500MB"), Ok(500_000_000));
        assert_eq!(parse_size("2gb"), Ok(2_000_000_000));
        assert_eq!(parse_size("1.5 TB"), Ok(1_500_000_000_000));
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5PB").is_err());
    }
}
