extern crate notcoal;

extern crate dirs;
extern crate ini;
extern crate structopt;

use notcoal::*;
use std::path::PathBuf;
use ini::Ini;
use structopt::StructOpt;
use std::process;

#[derive(StructOpt, Debug)]
#[structopt(name = "notcoal", about = "notmuch filters, not made from coal.")]
struct Opt {
    #[structopt(short = "c", long = "config", parse(from_os_str))]
    config: Option<PathBuf>,
    #[structopt(short = "f", long = "filters", parse(from_os_str))]
    filters: Option<PathBuf>,
    #[structopt(short = "t", long = "tag")]
    tag: Option<String>
}

pub fn get_db_path(config: &Option<PathBuf>) -> PathBuf {
    let mut p = dirs::home_dir().unwrap();
    let config = match config {
        Some(p) => p,
        None => {
            p.push(".notmuch-config");
            &p
        }
    };
    let db = Ini::load_from_file(config).unwrap();
    PathBuf::from(db.get_from(Some("database"), "path").unwrap())
}

pub fn get_filters(path: &Option<PathBuf>, db_path: &PathBuf) -> Vec<Filter> {
    let mut p = db_path.clone();
    let filter_path = match path {
        Some(p) => p,
        None => {
            p.push(".notmuch/hooks/notcoal-rules.json");
            &p
        }
    };
    match filters_from_file(filter_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{:?}", e);
            process::exit(1);
        }
    }
}

fn main() {
    let opt = Opt::from_args();
    let db_path = get_db_path(&opt.config);
    let filters = get_filters(&opt.filters, &db_path);
    let tag = match opt.tag {
        Some(t) => t,
        None => "new".to_string()
    };
    match filter_with_path(get_db_path(&None),
                           &format!("tag:{}", tag),
                           &filters) {
        Ok(_) => {
            println!("Yay you filtered your new messages");
        }
        Err(e) => {
            println!("Oops: {:?}", e);
        }
    };
}
