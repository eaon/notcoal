use clap::Parser;
use notcoal::*;
use notmuch::{ConfigKey, Database, DatabaseMode};
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
    /// Force maildir flag syncing  (overrides setting found in config)
    flags: Option<bool>,
    #[arg(long = "dry-run")]
    dry: bool,
}

pub fn get_maildir_sync_db(db: &Database) -> bool {
    match db.config_bool(ConfigKey::MaildirFlags) {
        Ok(bool) => bool,
        Err(err) => {
            eprintln!("Could not open notmuch database, aborting!");
            eprintln!("Error: {err}");
            process::exit(1);
        }
    }
}

pub fn get_filters(path: &Option<PathBuf>, db: &Database) -> Vec<Filter> {
    let mut p: PathBuf;
    let filter_path = match path {
        Some(p) => p,
        None => {
            p = match db.config(ConfigKey::HookDir) {
                Some(path) => PathBuf::from(path),
                None => {
                    eprintln!("Could not determine notmuch hooks directory, aborting!");
                    process::exit(1);
                }
            };
            p.push("notcoal-rules.json");
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

    let db = match Database::open_with_config::<&Path, _>(
        None,
        if opt.dry {
            DatabaseMode::ReadOnly
        } else {
            DatabaseMode::ReadWrite
        },
        opt.config,
        None,
    ) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("Could not open notmuch database, aborting!");
            eprintln!("Error: {err}");
            eprintln!("Do you have notmuch configured?");
            process::exit(1);
        }
    };

    let options = FilterOptions {
        sync_tags: match &opt.flags {
            Some(b) => *b,
            None => get_maildir_sync_db(&db),
        },
        leave_tag: opt.leave,
    };
    let filters = get_filters(&opt.filters, &db);

    if opt.dry {
        match filter_dry(&db, &opt.tag, &filters) {
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

    match filter(&db, &opt.tag, &options, &filters) {
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
