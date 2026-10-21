mod archive;
mod cli;
mod engine;
mod fsutil;
mod journal;
mod platform;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anstream::{eprintln, print, println};
use anstyle::{AnsiColor, Style};
use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use indicatif::{ProgressBar, ProgressStyle};

use archive::{Kind, storage};
use cli::{Cli, Command, EXIT_DAMAGED, EXIT_FAILURE, EXIT_NO_SPACE, EXIT_PAUSED, Space, Target};
use engine::{Job, Outcome, Progress, Run, human};
use journal::{Journal, Paths, Plan};

const ERROR: Style = AnsiColor::Red.on_default().bold();
const GOOD: Style = AnsiColor::Green.on_default().bold();
const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();

static STOP: AtomicBool = AtomicBool::new(false);
static QUIET: AtomicBool = AtomicBool::new(false);

/// Normal output, silenced by `--quiet`. Errors and prompts always show.
macro_rules! say {
    ($($t:tt)*) => {
        if !QUIET.load(Ordering::Relaxed) {
            println!($($t)*);
        }
    };
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    QUIET.store(cli.quiet, Ordering::Relaxed);
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{ERROR}error:{ERROR:#} {e:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Extract {
            target,
            verify_first,
            space,
            yes,
        } => extract(target, verify_first, space, yes),
        Command::Check { target, space } => check(target, space),
        Command::List { archive } => list(&archive),
        Command::Test { archive } => test_command(&archive),
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "hatchzip", &mut io::stdout());
            Ok(ExitCode::SUCCESS)
        }
        Command::Man => {
            clap_mangen::Man::new(Cli::command()).render(&mut io::stdout())?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The first volume of the archive and where its journal lives. Paths are
/// made absolute so a resume works from any directory.
fn locate(archive: &Path) -> Result<(PathBuf, Paths)> {
    let primary = std::path::absolute(storage::first_volume(archive))
        .with_context(|| format!("resolving {}", archive.display()))?;
    let paths = Paths::for_archive(&primary);
    Ok((primary, paths))
}

enum State {
    /// Nothing touched yet. Nothing changes on disk until `extract` decides
    /// to go ahead and writes the journal.
    Fresh(Job, PathBuf),
    Resume(Job, Journal),
    /// The archive is gone but the run died before cleaning up.
    Orphan(Journal),
}

/// `writable` is false for the commands that only look, so they work on
/// read-only media and never take the lock.
fn open(primary: &Path, paths: &Paths, output: Option<PathBuf>, writable: bool) -> Result<State> {
    if paths.exists() {
        let (journal, tail) = Journal::load(paths)?;
        if let Some(o) = &output
            && std::path::absolute(o)? != journal.output
        {
            bail!(
                "this archive is already half extracted into {}; finish it there first",
                journal.output.display()
            );
        }
        if !journal.parts[0].0.exists() {
            return Ok(State::Orphan(journal));
        }
        let job = Job::resume(&journal, tail, writable)?;
        return Ok(State::Resume(job, journal));
    }
    let job = Job::open(primary, writable)?;
    let output = match output {
        Some(o) => std::path::absolute(o)?,
        None => default_output(primary),
    };
    if output.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        bail!(
            "{} already exists and is not empty; pick another folder with -o",
            output.display()
        );
    }
    Ok(State::Fresh(job, output))
}

/// `game.zip` and `game.7z.001` both extract into `game`.
fn default_output(primary: &Path) -> PathBuf {
    let file_name = primary
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut name = file_name.clone();
    if let Some((stem, digits)) = name.rsplit_once('.')
        && digits.len() >= 3
        && digits.bytes().all(|b| b.is_ascii_digit())
    {
        name = stem.to_string();
    }
    let lower = name.to_ascii_lowercase();
    for ext in [".zip", ".7z", ".tar", ".exe"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            name.truncate(name.len() - ext.len());
            break;
        }
    }
    if name == file_name {
        name.push_str("-extracted");
    }
    primary.with_file_name(name)
}

fn display_name(job: &Job) -> String {
    let parts = job.storage.parts();
    let first = parts[0]
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match parts.len() {
        1 => first,
        n => format!("{first} (+{} more parts)", n - 1),
    }
}

/// Prints the space plan and returns whether it fits. The verdict prints
/// even under `--quiet`, since it is the whole answer.
fn report(job: &Job, out: &Path, journal: Option<&Journal>, keep_free: u64) -> Result<bool> {
    let reclaims = fsutil::same_filesystem(job.storage.primary(), out);
    let stale: u64 = job.uncommitted(out).iter().map(|(_, n)| n).sum();
    let free = fsutil::available_space(out)? + stale;
    let (peak, end) = job.space_needed(journal.is_none(), reclaims);
    let left = job.remaining_size();

    say!(
        "{BOLD}{}{BOLD:#}  {} {}, unpacks to {} in {} entries",
        display_name(job),
        human(job.storage.len()),
        job.format_name,
        human(journal.map_or(left, |j| j.total_size)),
        job.item_count()
    );
    if let Some(j) = journal {
        say!(
            "  resuming: {} of {} left",
            human(left),
            human(j.total_size)
        );
    }
    say!("  free space now        {}", human(free));
    say!("  normal extractors     {} free", human(left));
    say!(
        "  HatchZip needs        {BOLD}{}{BOLD:#} free {DIM}(+{} kept spare){DIM:#}",
        human(peak),
        human(keep_free)
    );
    if !reclaims {
        say!(
            "  {DIM}note: the output is on another disk, so deleting the archive won't make room there{DIM:#}"
        );
    }

    let fits = free >= peak + keep_free;
    if fits {
        let after = (free as i128 - end).max(0) as u64;
        say!("  free after            {}", human(after));
    } else {
        let short = human(peak + keep_free - free);
        println!("{ERROR}Not enough space.{ERROR:#} Free up {short} more, or lower --keep-free.");
        if job
            .units
            .iter()
            .any(|u| u.items.len() > 1 && u.size() > free)
        {
            println!(
                "{DIM}Most of that is one solid block, which can only be freed once all of it is \
                 extracted. Archives packed with smaller solid blocks need far less.{DIM:#}"
            );
        }
    }
    Ok(fits)
}

fn check(target: Target, space: Space) -> Result<ExitCode> {
    let (primary, paths) = locate(&target.archive)?;
    let fits = match open(&primary, &paths, target.output, false)? {
        State::Fresh(job, out) => report(&job, &out, None, space.keep_free)?,
        State::Resume(job, journal) => {
            report(&job, &journal.output, Some(&journal), space.keep_free)?
        }
        State::Orphan(_) => {
            say!("The archive is already fully extracted; run extract to finish cleaning up.");
            true
        }
    };
    Ok(if fits {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_NO_SPACE)
    })
}

fn list(archive: &Path) -> Result<ExitCode> {
    let (primary, paths) = locate(archive)?;
    let job = match open(&primary, &paths, None, false) {
        Ok(State::Fresh(job, _)) | Ok(State::Resume(job, _)) => job,
        Ok(State::Orphan(_)) => bail!("nothing is left in this archive"),
        // A non-empty folder of the same name only matters to extract.
        Err(_) if !paths.exists() => Job::open(&primary, false)?,
        Err(e) => return Err(e),
    };
    println!("{BOLD}{:>10}  Name{BOLD:#}", "Size");
    for item in job.units.iter().rev().flat_map(|u| &u.items) {
        let path = item.path.display();
        match &item.kind {
            Kind::Dir => println!("{:>10}  {path}/", ""),
            Kind::File => println!("{:>10}  {path}", human(item.size)),
            Kind::Symlink => println!("{:>10}  {path} {DIM}(symlink){DIM:#}", ""),
            Kind::HardLink(t) => {
                println!("{:>10}  {path} {DIM}(link to {}){DIM:#}", "", t.display())
            }
        }
    }
    say!(
        "{DIM}{} entries, {} unpacked, {} {} archive{DIM:#}",
        job.item_count(),
        human(job.remaining_size()),
        human(job.storage.len()),
        job.format_name
    );
    Ok(ExitCode::SUCCESS)
}

fn test_command(archive: &Path) -> Result<ExitCode> {
    let (primary, paths) = locate(archive)?;
    if paths.exists() {
        bail!("this archive is half extracted; run extract to continue it");
    }
    let job = Job::open(&primary, false)?;
    if let Err(e) = verify(&job) {
        eprintln!("{ERROR}error:{ERROR:#} {e:#}");
        return Ok(ExitCode::from(EXIT_DAMAGED));
    }
    say!("{GOOD}All {} entries are intact.{GOOD:#}", job.item_count());
    Ok(ExitCode::SUCCESS)
}

fn extract(target: Target, verify_first: bool, space: Space, yes: bool) -> Result<ExitCode> {
    let keep_free = space.keep_free;
    let (primary, paths) = locate(&target.archive)?;
    let (job, journal) = match open(&primary, &paths, target.output, true)? {
        State::Orphan(journal) => {
            engine::finish_orphaned(&journal, &paths)?;
            say!(
                "{GOOD}Done.{GOOD:#} Everything is in {}.",
                journal.output.display()
            );
            return Ok(ExitCode::SUCCESS);
        }
        State::Resume(job, journal) => {
            if !report(&job, &journal.output, Some(&journal), keep_free)? {
                return Ok(ExitCode::from(EXIT_NO_SPACE));
            }
            for (p, _) in job.uncommitted(&journal.output) {
                std::fs::remove_file(&p)
                    .with_context(|| format!("removing stale {}", p.display()))?;
            }
            (job, journal)
        }
        State::Fresh(job, out) => {
            if !report(&job, &out, None, keep_free)? {
                return Ok(ExitCode::from(EXIT_NO_SPACE));
            }
            if verify_first && let Err(e) = verify(&job) {
                eprintln!("{ERROR}error:{ERROR:#} {e:#}");
                return Ok(ExitCode::from(EXIT_DAMAGED));
            }
            if !yes {
                confirm(&display_name(&job), &out)?;
            }
            let plan = Plan {
                format: job.format_name,
                tail_start: job.tail_start,
                output: out,
                total_size: job.remaining_size(),
            };
            let journal = Journal::create(&paths, &job.storage, plan)?;
            (job, journal)
        }
    };

    ctrlc::set_handler(|| {
        if STOP.swap(true, Ordering::Relaxed) {
            std::process::exit(EXIT_PAUSED.into());
        }
        eprintln!("\nFinishing the current files, then pausing. Press Ctrl+C again to quit now.");
    })
    .context("installing Ctrl+C handler")?;

    let bar = progress_bar(journal.total_size, "Hatching");
    bar.set_position(journal.total_size - job.remaining_size());
    bar.set_message(format!("archive {}", human(journal.committed_len)));

    let output = journal.output.clone();
    let name = display_name(&job);
    let (b1, b2) = (bar.clone(), bar.clone());
    let run = Run {
        job,
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
            say!(
                "{GOOD}Done.{GOOD:#} Everything is in {}, and {name} is gone.",
                output.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(Outcome::Paused) => {
            say!("Paused. Run the same command again to continue.");
            Ok(ExitCode::from(EXIT_PAUSED))
        }
        Err(e) => {
            eprintln!("{ERROR}error:{ERROR:#} {e:#}");
            eprintln!("Progress is saved. Run the same command again to continue.");
            Ok(ExitCode::from(EXIT_FAILURE))
        }
    }
}

fn verify(job: &Job) -> Result<()> {
    let bar = progress_bar(job.remaining_size(), "Checking");
    let b = bar.clone();
    let result = job.verify(&move |n| b.inc(n));
    bar.finish_and_clear();
    result.context("the archive is damaged, so nothing was extracted")
}

fn confirm(archive: &str, output: &Path) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!("HatchZip deletes the archive as it extracts; pass --yes to confirm");
    }
    print!(
        "\nThis extracts into {} and deletes {BOLD}{archive}{BOLD:#} as it goes. Continue? [y/N] ",
        output.display()
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
    if QUIET.load(Ordering::Relaxed) {
        return ProgressBar::hidden();
    }
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
