extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

extern crate dirs;
extern crate ini;
extern crate notmuch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::io::Read;
use std::fs::File;

use ini::Ini;
use notmuch::{Database, DatabaseMode, StreamingIterator};

mod error;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Value {
    Single(String),
    Multiple(Vec<String>),
    Bool(bool)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    #[serde(rename="add")]
    Add(Value),
    #[serde(rename="rm")]
    Rm(Value),
    #[serde(rename="run")]
    Run(String)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub rules: Vec<HashMap<String, Value>>,
    pub op: Operation
}

impl Filter {
    fn validate(&self) -> bool {
        true
    }
}

pub fn filter(db: &Path, filters: Vec<Filter>) -> Result<(), error::Error> {
    let db = Database::open(&db, DatabaseMode::ReadWrite).unwrap();
    let q = db.create_query("tag:new").unwrap();
    let mut msgs = q.search_messages().unwrap();
    while let Some(msg) = msgs.next() {
        println!();
    }
    Ok(())
}

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

pub fn filters_from(buf: &[u8]) -> Result<Vec<Filter>, error::Error> {
    match serde_json::from_slice::<Vec<Filter>>(&buf) {
        Ok(j) => Ok(j),
        Err(e) => {
            println!("{:?}", e);
            Err(error::Error::UnspecifiedError)
        }
    }
}

pub fn filters_from_file(filename: &str) -> Result<Vec<Filter>, error::Error> {
    let mut buf = Vec::new();
    File::open(filename).unwrap().read_to_end(&mut buf).unwrap();
    filters_from(&buf)
}
