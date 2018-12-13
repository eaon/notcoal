extern crate notcoal;

extern crate dirs;
extern crate ini;
extern crate structopt;

use ini::Ini;
use notcoal::*;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(name = "notcoal", about = "notmuch filters, not made from coal.")]
struct Opt {
    #[structopt(short = "c", long = "config", parse(from_os_str))]
    /// [default: ~/.notmuch-config]
    config: Option<PathBuf>,
    #[structopt(short = "f", long = "filters", parse(from_os_str))]
    /// [default: ~/$notmuchdb/.notmuch/hooks/notcoal-rules.json]
    filters: Option<PathBuf>,
    #[structopt(short = "t", long = "tag", default_value = "new")]
    tag: String,
    #[structopt(long = "dry-run")]
    dry: bool,
}

pub fn get_db_path(config: &Option<PathBuf>) -> Option<PathBuf> {
    let mut p: PathBuf;
    let config = match config {
        Some(p) => p,
        None => {
            p = dirs::home_dir()?;
            p.push(".notmuch-config");
            &p
        }
    };
    let db = match Ini::load_from_file(config) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}\nDo you have notmuch configured?", e);
            process::exit(1);
        }
    };
    Some(PathBuf::from(db.get_from(Some("database"), "path")?))
}

pub fn get_filters(path: &Option<PathBuf>, db_path: &PathBuf) -> Vec<Filter> {
    let mut p: PathBuf;
    let filter_path = match path {
        Some(p) => p,
        None => {
            p = db_path.clone();
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
    let opt = Opt::from_args();
    let db_path = match get_db_path(&opt.config) {
        Some(db) => db,
        None => {
            eprintln!("Can't get the database path");
            process::exit(1);
        }
    };
    let filters = get_filters(&opt.filters, &db_path);

    if opt.dry {
        match filter_dry_with_path(&db_path, &opt.tag, &filters) {
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

    match filter_with_path(&db_path, &opt.tag, &filters) {
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
