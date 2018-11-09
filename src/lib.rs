extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate regex;
extern crate notmuch;

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::collections::HashMap;
use std::collections::BTreeMap;
use std::iter::Iterator;
use std::convert::AsRef;
use std::path::{Path};
use std::io::Read;
use std::fs::File;
use std::process::{Command, Stdio};

use regex::Regex;

use notmuch::{Database, DatabaseMode, StreamingIterator, Message,
              MessageOwner};

pub mod error;
use error::Error;
use error::Result;

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
pub struct Operation {
    pub rm: Option<Value>,
    pub add: Option<Value>,
    pub run: Option<Vec<String>>
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    name: Option<String>,
    pub desc: Option<String>,
    // at the moment, since we are generating a hash in the get_name function
    // this field needs to be consistent in the order it prints its key/value
    // pairs
    pub rules: Vec<BTreeMap<String, Value>>,
    pub op: Operation,
    #[serde(skip)]
    re: Vec<HashMap<String, Vec<Regex>>>
}

impl Filter {
    pub fn new() -> Filter {
        Filter {
            name: None,
            desc: None,
            rules: Vec::new(),
            op: Operation { rm: None, add: None, run: None },
            re: Vec::new()
        }
    }

    pub fn get_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => {
                // XXX This seems dumb, there has to be a better way
                let mut h = DefaultHasher::new();
                let buf = format!("{:?}", self.rules);
                for byte in buf.as_bytes() {
                    h.write_u8(*byte);
                }
                format!("{:x}", h.finish())
            }
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = Some(name.to_string());
    }

    pub fn compile(mut self) -> Result<Self> {
        for rule in &self.rules {
            let mut compiled = HashMap::new();
            for (key, value) in rule.iter() {
                let mut res = Vec::new();
                match value {
                    Value::Single(re) => res.push(match Regex::new(&re) {
                        Ok(re) => re,
                        Err(err) => return Err(Error::RegexError(err))
                    }),
                    Value::Multiple(mre) => {
                        for re in mre {
                            res.push(match Regex::new(&re) {
                                Ok(re) => re,
                                Err(err) => return Err(Error::RegexError(err))
                            });
                        }
                    }
                    _ => {
                        let e = "Not a regular expression".to_string();
                        let e = Error::RegexError(regex::Error::Syntax(e));
                        return Err(e);
                    }
                }
                compiled.insert(key.to_string(), res);
            }
            self.re.push(compiled);
        }
        Ok(self)
    }

    pub fn apply_if_match<T>(&self, msg: &Message<T>) -> Result<bool>
        where T: MessageOwner {
        if self.is_match(msg) {
            match self.apply(msg) {
                Ok(_) => Ok(true),
                Err(e) => Err(e)
            }
        } else {
            Ok(false)
        }
    }

    pub fn is_match<T>(&self, msg: &Message<T>) -> bool
        where T: MessageOwner {

        fn sub_match<I>(res: &[Regex], values: I) -> bool
        where I: Iterator<Item=String> {
            for value in values {
                for re in res {
                    if re.is_match(&value) {
                        return true;
                    }
                }
            }
            false
        }

        for rule in &self.re {
            let mut is_match = true;
            for (part, res) in rule {
                if part == "@path" {
                    let values = msg.filenames()
                                    .filter_map(|f| {
                                        match f.to_str() {
                                            Some(n) => Some(n.to_string()),
                                            None => None,
                                        }
                                    });
                    is_match = sub_match(&res, values) && is_match;
                } else if part == "@tags" {
                    is_match = sub_match(&res, msg.tags()) && is_match;
                }
                if part.starts_with('@') {
                    continue;
                }

                match msg.header(part) {
                    Ok(None) => {
                        is_match = false;
                    }
                    Ok(Some(p)) => {
                        for re in res {
                            is_match = re.is_match(p) && is_match;
                            if ! is_match {
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        // log warning should go here but we probably don't
                        // care, since requesting a header most likely won't
                        // trash the whole database.
                    }
                }
            }
            if is_match {
                return true;
            }
        }
        false
    }

    pub fn apply<T>(&self, msg: &Message<T>) -> Result<()>
        where T: MessageOwner {
        use Value::*;
        if let Some(rm) = &self.op.rm {
            match rm {
                Single(tag) => {
                    msg.remove_tag(tag);
                }
                Multiple(tags) => {
                    for tag in tags {
                        msg.remove_tag(tag);
                    }
                }
                Bool(all) => {
                    if *all {
                        msg.remove_all_tags();
                    }
                }
            }
        }
        if let Some(add) = &self.op.add {
            match add {
                Single(ref tag) => {
                    msg.add_tag(tag);
                }
                Multiple(ref tags) => {
                    for tag in tags {
                        msg.add_tag(tag);
                    }
                }
                Bool(_) => {
                    return Err(Error::UnspecifiedError);
                }
            }
        }
        if let Some(argv) = &self.op.run {
            Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdout(Stdio::inherit())
                    .env("NOTCOAL_FILE_NAME", &msg.filename())
                    .env("NOTCOAL_MSG_ID", &msg.id())
                    .env("NOTCOAL_FILTER_NAME", &self.get_name())
                    .spawn()
                    .expect(&format!("'{:?}' couldn't be started", argv));
        }
        Ok(())
    }
}

pub fn filter(db: &Database, query_tag: &str,
              filters: &[Filter]) -> Result<usize> {
    let q = db.create_query(&format!("tag:{}", query_tag)).unwrap();
    let mut msgs = q.search_messages().unwrap();
    let mut matches = 0;
    while let Some(msg) = msgs.next() {
        for filter in filters {
            match filter.apply_if_match(&msg) {
                Ok(_) => matches += 1,
                Err(e) => return Err(e)
            }
        }
        msg.remove_tag(query_tag);
    }
    Ok(matches)
}

pub fn filter_dry(db: &Database, query_tag: &str,
                  filters: &[Filter]) ->  Result<(usize, Vec<String>)> {
    let q = db.create_query(&format!("tag:{}", query_tag)).unwrap();
    let mut msgs = q.search_messages().unwrap();
    let mut matches = 0;
    let mut mtchinf = Vec::<String>::new();
    while let Some(msg) = msgs.next() {
        matches += filters.iter().fold(0, |mut a, f| {
            if f.is_match(&msg) {
                mtchinf.push(format!("{}: {}", msg.id(), f.get_name()));
                a += 1
            }
            a
        });
    }
    Ok((matches, mtchinf))
}

pub fn filter_with_path<P>(db: P, query: &str,
                           filters: &[Filter]) -> Result<usize>
    where P: AsRef<Path> {
    let db = Database::open(&db, DatabaseMode::ReadWrite).unwrap();
    filter(&db, query, filters)
}

pub fn filter_dry_with_path<P>(db: P, query: &str,
                           filters: &[Filter]) -> Result<(usize, Vec<String>)>
    where P: AsRef<Path> {
    let db = Database::open(&db, DatabaseMode::ReadWrite).unwrap();
    filter_dry(&db, query, filters)
}

pub fn filters_from(buf: &[u8]) -> Result<Vec<Filter>> {
    // XXX What even is this, how can I track the or return an error more
    // efficiently here?
    match serde_json::from_slice::<Vec<Filter>>(&buf) {
        Ok(j) => {
            let mut error: Result<Filter> = Ok(Filter::new());
            let fs = j.into_iter()
                      .filter_map(|f| {
                          match f.compile() {
                              Ok(f) => Some(f),
                              Err(e) => {
                                  error = Err(e);
                                  None
                              },
                          }
                      })
                      .collect();
            match error {
                Ok(_) => {},
                Err(e) => return Err(e),
            }
            Ok(fs)
        },
        Err(e) => return Err(Error::JSONError(e)),
    }
}

pub fn filters_from_file<P>(filename: &P) -> Result<Vec<Filter>>
    where P: AsRef<Path> {
    let mut buf = Vec::new();
    let mut file = match File::open(filename) {
        Ok(f) => f,
        Err(e) => return Err(Error::IoError(e)),
    };
    match file.read_to_end(&mut buf) {
        Ok(_) => {},
        Err(e) => return Err(Error::IoError(e)),
    }
    filters_from(&buf)
}
