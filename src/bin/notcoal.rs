use clap::Parser;
use ini::Ini;
use notcoal::*;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Parser, Debug)]
#[command(name = "notcoal", about = "notmuch filters, not made from coal.")]
struct Opt {
    #[arg(short, long = "config")]
    /// Configuration file [default: ~/.notmuch-config]
    config: Option<PathBuf>,
    #[arg(short, long = "filters")]
    /// Rule file [default: ~/$notmuchdb/.notmuch/hooks/notcoal-rules.json]
    filters: Option<PathBuf>,
    #[arg(short, long = "tag", default_value = "new")]
    /// Tag to query
    tag: String,
    #[arg(long = "leave-tag")]
    /// Leave the "query tag" in place instead of removing once all filters ran
    leave: bool,
    #[arg(long = "sync-flags")]
    /// Force maildir flag syncing  (overrides setting found in config) [true |
    /// false]
    flags: Option<bool>,
    #[arg(long = "dry-run")]
    dry: bool,
}

pub fn get_config(config: &Option<PathBuf>) -> Ini {
    let mut p: PathBuf;
    let config = match config {
        Some(p) => p,
        None => {
            p = match dirs::home_dir() {
                Some(h) => h,
                None => {
                    eprintln!("Cannot determine home directory, aborting");
                    process::exit(1);
                }
            };
            p.push(".notmuch-config");
            &p
        }
    };
    match Ini::load_from_file(config) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}\nDo you have notmuch configured?", e);
            process::exit(1);
        }
    }
}

pub fn get_maildir_sync(config: &Ini) -> bool {
    matches!(
        config.get_from(Some("maildir"), "synchronize_flags"),
        Some("true")
    )
}

pub fn get_db_path(config: &Ini) -> Option<PathBuf> {
    Some(PathBuf::from(config.get_from(Some("database"), "path")?))
}

pub fn get_filters(path: &Option<PathBuf>, db_path: &Path) -> Vec<Filter> {
    let mut p: PathBuf;
    let filter_path = match path {
        Some(p) => p,
        None => {
            p = db_path.to_path_buf();
            p.push(".notmuch/hooks/notcoal-rules.json");
            &p
        }
    };
    match filters_from_file(filter_path) {
        Ok(f) => f,
        Err(e) => {
            // using {} here results in stack overflow when getting a JSONError…
            eprintln!("Couldn't load filters: {:?}", e);
            process::exit(1);
        }
    }
}

fn main() {
    let opt = Opt::parse();
    let config = get_config(&opt.config);
    let db_path = match get_db_path(&config) {
        Some(db) => db,
        None => {
            eprintln!("Can't get the database path");
            process::exit(1);
        }
    };

    let options = FilterOptions {
        sync_tags: match &opt.flags {
            Some(b) => *b,
            None => get_maildir_sync(&config),
        },
        leave_tag: opt.leave,
    };
    let filters = get_filters(&opt.filters, &db_path);

    if opt.dry {
        match filter_dry_with_path::<PathBuf, PathBuf>(&db_path, &opt.tag, &filters) {
            Ok(m) => {
                println!("There are {} matches:", m.0);
                for info in m.1 {
                    println!("{}", info);
                }
            }
            Err(e) => {
                eprintln!("Oops: {}", e);
                process::exit(1);
            }
        }
        process::exit(0);
    }

    match filter_with_path::<PathBuf, PathBuf>(&db_path, &opt.tag, &options, &filters) {
        Ok(m) => {
            if m > 0 {
                println!("Yay you successfully applied {} filters", m);
            } else {
                println!("No message filtering necessary!");
            }
        }
        Err(e) => {
            eprintln!("Oops: {}", e);
            process::exit(1);
        }
    };
}
