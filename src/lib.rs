extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate regex;
extern crate notmuch;

use std::collections::HashMap;
use std::iter::Iterator;
use std::convert::AsRef;
use std::path::{Path};
use std::io::Read;
use std::fs::File;

use regex::Regex;

use notmuch::{Database, DatabaseMode, StreamingIterator, Message,
              MessageOwner};

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
pub struct Operation {
    rm: Option<Value>,
    add: Option<Value>,
    run: Option<String>
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    name: Option<String>,
    desc: Option<String>,
    rules: Vec<HashMap<String, Value>>,
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
                    _ => {
                        let e = "Not a regular expression".to_string();
                        return Err(regex::Error::Syntax(e));
                    }
                }
                compiled.insert(key.to_string(), res);
            }
            self.re.push(compiled);
        }
        Ok(self)
    }

    fn apply_if_match<T: MessageOwner>(&self, msg: &Message<T>) ->
       Result<bool, error::Error> {
        if self.is_match(msg) {
            match self.apply(msg) {
                Ok(_) => Ok(true),
                Err(e) => Err(e)
            }
        } else {
            Ok(false)
        }
    }

    fn is_match<T: MessageOwner>(&self, msg: &Message<T>) -> bool {
        for rule in &self.re {
            // XXX: The @special features ought to be handled in other more
            // generalised functions. Avoid code duplication etc.
            let mut is_match = true;
            for (part, res) in rule {
                if part == "@folder" {
                    let values = msg.filenames()
                                    .map(|f| f.to_str()
                                              .unwrap()
                                              .to_string());
                    is_match = sub_match(&res, values) && is_match;
                } else if part == "@tags" {
                    is_match = sub_match(&res, msg.tags()) && is_match;
                }
                if part.starts_with('@') {
                    continue;
                }
                match msg.header(part) {
                    Ok("") => {
                        is_match = false;
                    }
                    Ok(p) => {
                        for re in res {
                            is_match = re.is_match(p) && is_match;
                            if ! is_match {
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        // log warning should go here but we probably don't
                        // care
                    }
                }
            }
            if is_match {
                return true;
            }
        }
        false
    }

    fn apply<T: MessageOwner>(&self, msg: &Message<T>) ->
       Result<(), error::Error> {
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
                    return Err(error::Error::UnspecifiedError);
                }
            }
        }
        if let Some(_run) = &self.op.run {
            // Not yet implemented
        }
        Ok(())
    }
}

fn sub_match<I: Iterator<Item=String>>(res: &[Regex], values: I) -> bool {
    for value in values {
        for re in res {
            if re.is_match(&value) {
                return true;
            }
        }
    }
    false
}

pub fn filter(db: &Database, query_tag: &str, filters: &[Filter]) ->
       Result<(), error::Error> {
    let q = db.create_query(&format!("tag:{}", query_tag)).unwrap();
    let mut msgs = q.search_messages().unwrap();
    while let Some(msg) = msgs.next() {
        for filter in filters {
            match filter.apply_if_match(&msg) {
                Ok(_) => {},
                Err(e) => return Err(e)
            }
        }
        msg.remove_tag(query_tag);
    }
    Ok(())
}

pub fn filter_with_path<P: AsRef<Path>>(db: P, query: &str,
                                        filters: &[Filter]) ->
       Result<(), error::Error> {
    let db = Database::open(&db, DatabaseMode::ReadWrite).unwrap();
    filter(&db, query, filters)
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

pub fn filters_from_file<P: AsRef<Path>>(filename: &P) ->
       Result<Vec<Filter>, error::Error> {
    let mut buf = Vec::new();
    File::open(filename).unwrap().read_to_end(&mut buf).unwrap();
    filters_from(&buf)
}
