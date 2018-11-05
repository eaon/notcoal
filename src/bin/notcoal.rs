extern crate notcoal;

extern crate dirs;
extern crate ini;

use std::path::PathBuf;

use ini::Ini;

pub fn get_db_path(path: Option<PathBuf>) -> PathBuf {
    let config = match path {
        Some(p) => p,
        None => {
            let mut p = dirs::home_dir().unwrap();
            p.push(".notmuch-config");
            p
        }
    };
    let db = Ini::load_from_file(config).unwrap();
    PathBuf::from(db.get_from(Some("database"), "path").unwrap())
}

fn main() {
    let filters = match notcoal::filters_from_file("examples/rules.json") {
        Ok(f) => f,
        Err(e) => {
            println!("{:?}", e);
            Vec::new()
        }
    };

    match notcoal::filter_with_path(get_db_path(None), "tag:new", &filters) {
        Ok(_) => {
            println!("Yay you filtered your new messages");
        }
        Err(e) => {
            println!("Oops: {:?}", e);
        }
    };
}
