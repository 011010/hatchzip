mod engine;
mod fsutil;
mod journal;
mod platform;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use engine::{Archive, Outcome, Progress, Run, human};
use journal::{Journal, Paths};

#[derive(Parser)]
#[command(
    name = "hatchzip",
    version,
    about = "Extract huge archives in place: the archive shrinks as your files hatch."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract an archive, deleting it as it goes. Run it again after an
    /// interruption to continue where it stopped.
    #[command(alias = "x")]
    Extract {
        archive: PathBuf,
        /// Where to put the files [default: a folder named after the archive]
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Read the whole archive and check every file before touching it
        #[arg(long)]
        verify_first: bool,
        /// Free space to leave untouched, e.g. 500MB or 10GB
        #[arg(long, default_value = "2GB", value_parser = parse_size)]
        keep_free: u64,
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Show whether an archive fits on this disk, without changing anything
    Check {
        archive: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "2GB", value_parser = parse_size)]
        keep_free: u64,
    },
    /// Check every file in an archive for damage, without writing anything
    #[command(alias = "t")]
    Test { archive: PathBuf },
}

static STOP: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Extract {
            archive,
            output,
            verify_first,
            keep_free,
            yes,
        } => extract(&archive, output, verify_first, keep_free, yes),
        Command::Check {
            archive,
            output,
            keep_free,
        } => {
            let paths = Paths::for_archive(&archive);
            let (archive, journal) = open(&archive, &paths, output)?;
            Ok(if report(&archive, &journal, keep_free)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Test { archive } => {
            let archive = Archive::open(&archive)?;
            test(&archive)?;
            println!("All {} entries are intact.", archive.entries.len());
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Opens a fresh archive, or one a previous run already started on. The
/// journal is not written yet for a fresh one, so nothing changes on disk
/// until `extract` decides to go ahead.
fn open(path: &Path, paths: &Paths, output: Option<PathBuf>) -> Result<(Archive, Journal)> {
    if paths.exists() {
        let (journal, tail) = Journal::load(paths)?;
        if let Some(o) = &output
            && fsutil_abs(o) != fsutil_abs(&journal.output)
        {
            bail!(
                "this archive is already half extracted into {}; finish it there first",
                journal.output.display()
            );
        }
        let archive = Archive::reopen(path, &journal, tail)?;
        return Ok((archive, journal));
    }
    let archive = Archive::open(path)?;
    let output = output.unwrap_or_else(|| default_output(path));
    if output.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        bail!(
            "{} already exists and is not empty; pick another folder with -o",
            output.display()
        );
    }
    let journal = Journal {
        version: 1,
        archive_len: archive.len,
        tail_start: archive.tail_start,
        tail_crc: 0,
        output,
        done: 0,
        committed_len: archive.len,
        symlinks: Vec::new(),
    };
    Ok((archive, journal))
}

fn fsutil_abs(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn default_output(archive: &Path) -> PathBuf {
    let stem = archive.file_stem().unwrap_or_default();
    let parent = archive.parent().unwrap_or(Path::new(""));
    if archive.extension().is_some() {
        parent.join(stem)
    } else {
        let mut name = stem.to_owned();
        name.push("-extracted");
        parent.join(name)
    }
}

/// Prints the space plan and returns whether it fits.
fn report(archive: &Archive, journal: &Journal, keep_free: u64) -> Result<bool> {
    let out = &journal.output;
    let reclaims = fsutil::same_filesystem(&archive.path, out);
    let stale: u64 = archive
        .uncommitted(journal.done, out)
        .iter()
        .map(|(_, n)| n)
        .sum();
    let free = fsutil::available_space(out)? + stale;
    let (peak, end) = archive.space_needed(journal.done, reclaims);
    let total = archive.total_size();
    let left = archive.entries[journal.done..]
        .iter()
        .map(|e| e.size)
        .sum::<u64>();

    println!(
        "{}  {}, unpacks to {} in {} entries",
        archive.path.display(),
        human(archive.len),
        human(total),
        archive.entries.len()
    );
    if journal.done > 0 {
        println!("  resuming: {} of {} left", human(left), human(total));
    }
    println!("  free space now        {}", human(free));
    println!("  normal extractors     {} free", human(left));
    println!(
        "  HatchZip needs        {} free (+{} kept spare)",
        human(peak),
        human(keep_free)
    );
    if !reclaims {
        println!(
            "  note: the output is on another disk, so deleting the archive won't make room there"
        );
    }

    let fits = free >= peak + keep_free;
    if fits {
        let after = (free as i128 - end).max(0) as u64;
        println!("  free after            {}", human(after));
    } else {
        println!();
        println!(
            "Not enough space. Free up {} more, or lower --keep-free.",
            human(peak + keep_free - free)
        );
    }
    Ok(fits)
}

fn extract(
    path: &Path,
    output: Option<PathBuf>,
    verify_first: bool,
    keep_free: u64,
    yes: bool,
) -> Result<ExitCode> {
    let paths = Paths::for_archive(path);
    let resuming = paths.exists();
    let (mut archive, mut journal) = open(path, &paths, output)?;

    if !report(&archive, &journal, keep_free)? {
        return Ok(ExitCode::FAILURE);
    }
    if verify_first && !resuming {
        test(&archive)?;
    }
    if !resuming && !yes {
        confirm(&archive.path, &journal.output)?;
    }
    if resuming {
        for (p, _) in archive.uncommitted(journal.done, &journal.output) {
            std::fs::remove_file(&p).with_context(|| format!("removing stale {}", p.display()))?;
        }
    } else {
        journal = Journal::create(
            &paths,
            &mut archive.file,
            archive.tail_start,
            journal.output,
        )?;
    }

    ctrlc::set_handler(|| {
        if STOP.swap(true, Ordering::Relaxed) {
            std::process::exit(130);
        }
        eprintln!("\nFinishing the current files, then pausing. Press Ctrl+C again to quit now.");
    })
    .context("installing Ctrl+C handler")?;

    let done_bytes: u64 = archive.entries[..journal.done].iter().map(|e| e.size).sum();
    let bar = progress_bar(archive.total_size(), "Hatching");
    bar.set_position(done_bytes);
    bar.set_message(format!("archive {}", human(journal.committed_len)));

    let output = journal.output.clone();
    let name = archive.path.display().to_string();
    let count = archive.entries.len();
    let (b1, b2) = (bar.clone(), bar.clone());
    let run = Run {
        archive,
        journal,
        paths,
        stop: &STOP,
        progress: Progress {
            on_bytes: Box::new(move |n| b1.inc(n)),
            on_commit: Box::new(move |len| b2.set_message(format!("archive {}", human(len)))),
        },
        keep_free,
    };
    let outcome = run.execute();
    bar.finish_and_clear();

    match outcome {
        Ok(Outcome::Finished) => {
            println!(
                "Done. {count} entries are in {}, and {name} is gone.",
                output.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(Outcome::Paused) => {
            println!("Paused. Run the same command again to continue.");
            Ok(ExitCode::from(130))
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            eprintln!("Progress is saved. Run the same command again to continue.");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn test(archive: &Archive) -> Result<()> {
    let bar = progress_bar(archive.total_size(), "Checking");
    let b = bar.clone();
    let result = engine::verify(archive, &move |n| b.inc(n));
    bar.finish_and_clear();
    result.context("the archive is damaged, so nothing was extracted")
}

fn confirm(archive: &Path, output: &Path) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!("HatchZip deletes the archive as it extracts; pass --yes to confirm");
    }
    print!(
        "\nThis extracts into {} and deletes {} as it goes. Continue? [y/N] ",
        output.display(),
        archive.display()
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        bail!("cancelled; nothing was changed");
    }
    Ok(())
}

fn progress_bar(total: u64, prefix: &'static str) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template(
            "{prefix} [{bar:32}] {binary_bytes}/{binary_total_bytes}  {binary_bytes_per_sec}  eta {eta}  {msg}",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );
    bar.set_prefix(prefix);
    bar.enable_steady_tick(Duration::from_millis(200));
    bar
}

fn parse_size(s: &str) -> Result<u64, String> {
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
        _ => return Err(format!("unknown unit in '{s}'")),
    };
    Ok((num * mult) as u64)
}
