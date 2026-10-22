//! The command line. File manager menus call these exact forms, so changing
//! one breaks every installed right-click entry.

use std::path::PathBuf;

use clap::{ArgGroup, Parser};

#[derive(Parser, Debug)]
#[command(
    name = "hatchzip-gui",
    version,
    about = "Extract huge archives in place: the archive shrinks as your files hatch.",
    group(ArgGroup::new("what").args(["archive", "extract_here", "extract_to", "test"]))
)]
pub struct Args {
    /// The archive to open, or any part of a split archive
    pub archive: Option<PathBuf>,
    /// Extract into the folder the archive is in
    #[arg(long, value_name = "ARCHIVE")]
    pub extract_here: Option<PathBuf>,
    /// Extract into a new folder named after the archive
    #[arg(long, value_name = "ARCHIVE")]
    pub extract_to: Option<PathBuf>,
    /// Check every file for damage, without writing anything
    #[arg(long, value_name = "ARCHIVE")]
    pub test: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    ExtractHere,
    ExtractTo,
    Test,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    /// The main window, empty or with an archive open.
    Main(Option<PathBuf>),
    /// A small window that runs one task, started from a right-click.
    Task(TaskKind, PathBuf),
}

impl Args {
    pub fn launch(self) -> Launch {
        if let Some(a) = self.extract_here {
            Launch::Task(TaskKind::ExtractHere, a)
        } else if let Some(a) = self.extract_to {
            Launch::Task(TaskKind::ExtractTo, a)
        } else if let Some(a) = self.test {
            Launch::Task(TaskKind::Test, a)
        } else {
            Launch::Main(self.archive)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> Result<Launch, clap::Error> {
        let mut full = vec!["hatchzip-gui"];
        full.extend_from_slice(args);
        Args::try_parse_from(full).map(Args::launch)
    }

    #[test]
    fn command_line_is_well_formed() {
        Args::command().debug_assert();
    }

    #[test]
    fn every_documented_form_parses() {
        assert_eq!(parse(&[]).unwrap(), Launch::Main(None));
        assert_eq!(
            parse(&["game.zip"]).unwrap(),
            Launch::Main(Some("game.zip".into()))
        );
        assert_eq!(
            parse(&["--extract-here", "a b/game.7z.001"]).unwrap(),
            Launch::Task(TaskKind::ExtractHere, "a b/game.7z.001".into())
        );
        assert_eq!(
            parse(&["--extract-to", "game.zip"]).unwrap(),
            Launch::Task(TaskKind::ExtractTo, "game.zip".into())
        );
        assert_eq!(
            parse(&["--test", "game.tar"]).unwrap(),
            Launch::Task(TaskKind::Test, "game.tar".into())
        );
        assert_eq!(
            parse(&["--extract-here=game.zip"]).unwrap(),
            Launch::Task(TaskKind::ExtractHere, "game.zip".into())
        );
    }

    #[test]
    fn one_thing_at_a_time() {
        assert!(parse(&["--test", "a.zip", "--extract-to", "b.zip"]).is_err());
        assert!(parse(&["a.zip", "--extract-here", "b.zip"]).is_err());
        assert!(parse(&["--extract-here"]).is_err());
        assert!(parse(&["a.zip", "b.zip"]).is_err());
    }
}
