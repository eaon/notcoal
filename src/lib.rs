extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate regex;
extern crate notmuch;

use std::collections::HashMap;
use std::convert::AsRef;
use std::path::{Path};
use std::io::Read;
use std::fs::File;

use regex::Regex;

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
    pub op: Operation,
    #[serde(skip)]
    re: Vec<HashMap<String, Vec<Regex>>>
}

impl Filter {
    fn compile(mut self) -> Result<Self, regex::Error> {
        for rule in &self.rules {
            let mut compiled = HashMap::new();
            for (key, value) in rule.iter() {
                let mut res = Vec::new();
                match value {
                    Value::Single(re) => res.push(match Regex::new(&re) {
                        Ok(re) => re,
                        Err(err) => return Err(err)
                    }),
                    Value::Multiple(mre) => {
                        for re in mre {
                            res.push(match Regex::new(&re) {
                                Ok(re) => re,
                                Err(err) => return Err(err)
                            });
                        }
                    }
                    _ => return Err(regex::Error::Syntax("Not a regular expression".to_string()))
                }
                compiled.insert(key.to_string(), res);
            }
            self.re.push(compiled);
        }
        println!("{:?}", self.re);
        Ok(self)
    }
}

pub fn filter_with_db(db: &Database, query: &str, filter: Vec<Filter>)
       -> Result<(), error::Error> {
    let q = db.create_query(query).unwrap();
    let mut msgs = q.search_messages().unwrap();
    while let Some(msg) = msgs.next() {
        println!();
    }
    Ok(())
}

pub fn filter(db: &Path, query: &str, filters: Vec<Filter>) -> Result<(), error::Error> {
    let db = Database::open(&db, DatabaseMode::ReadWrite).unwrap();
    filter_with_db(&db, query, filters)
}

pub fn filters_from(buf: &[u8]) -> Result<Vec<Filter>, error::Error> {
    match serde_json::from_slice::<Vec<Filter>>(&buf) {
        Ok(j) => {
            Ok(j.into_iter()
                .map(|f| f.compile().unwrap())
                .collect())
        },
        Err(e) => {
            println!("{:?}", e);
            Err(error::Error::UnspecifiedError)
        }
    }
}

pub fn filters_from_file<P: AsRef<Path>>(filename: P) -> Result<Vec<Filter>, error::Error> {
    let mut buf = Vec::new();
    File::open(filename).unwrap().read_to_end(&mut buf).unwrap();
    filters_from(&buf)
}
