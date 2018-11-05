extern crate notcoal;

extern crate dirs;
extern crate ini;

use std::path::PathBuf;

use ini::Ini;

use notcoal::Value::*;
use notcoal::Operation;

pub fn load_config(path: Option<PathBuf>) {
    let config = match path {
        Some(p) => p,
        None => {
            let mut p = dirs::home_dir().unwrap();
            p.push(".notmuch-config");
            p
        }
    };
    let db = Ini::load_from_file(config).unwrap();
    println!("{:#?}", PathBuf::from(db.get_from(Some("database"), "path").unwrap()));
}

fn main() {
    let filters = match notcoal::filters_from_file("example-rules.json") {
        Ok(f) => f,
        Err(e) => {
            println!("{:?}", e);
            Vec::new()
        }
    };

    for filter in filters {
        match filter.op {
            Operation::Rm(Single(ref tag)) => println!("{}", tag),
            Operation::Add(Single(ref tag)) => println!("{}", tag),
            Operation::Rm(Multiple(ref tags)) => {
                for tag in tags {
                    println!("{}", tag);
                }
            },
            Operation::Rm(Bool(ref all)) => println!("remove all tags: {:?}", all),
            _ => {}
        }
    }
}
