extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate notmuch;
extern crate regex;

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::convert::AsRef;
use std::fs::File;
use std::hash::Hasher;
use std::io::Read;
use std::iter::Iterator;
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;

use notmuch::{
    Database, DatabaseMode, Message, MessageOwner, StreamingIterator,
};

pub mod error;
use error::Error::*;
use error::Result;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Value {
    Single(String),
    Multiple(Vec<String>),
    Bool(bool),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub rm: Option<Value>,
    pub add: Option<Value>,
    pub run: Option<Vec<String>>,
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
    re: Vec<HashMap<String, Vec<Regex>>>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            name: None,
            desc: None,
            rules: Vec::new(),
            op: Operation {
                rm: None,
                add: None,
                run: None,
            },
            re: Vec::new(),
        }
    }
}

impl Filter {
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
        use Value::*;

        for rule in &self.rules {
            let mut compiled = HashMap::new();
            for (key, value) in rule.iter() {
                let mut res = Vec::new();
                match value {
                    Single(re) => res.push(Regex::new(&re)?),
                    Multiple(mre) => {
                        for re in mre {
                            res.push(Regex::new(&re)?);
                        }
                    }
                    _ => {
                        let e = "Not a regular expression".to_string();
                        let e = RegexError(regex::Error::Syntax(e));
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
    where
        T: MessageOwner,
    {
        if self.is_match(msg) {
            Ok(self.apply(msg)?)
        } else {
            Ok(false)
        }
    }

    pub fn is_match<T>(&self, msg: &Message<T>) -> bool
    where
        T: MessageOwner,
    {
        fn sub_match<I>(res: &[Regex], values: I) -> bool
        where
            I: Iterator<Item = String>,
        {
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
                    let vs = msg.filenames().filter_map(|f| match f.to_str() {
                        Some(n) => Some(n.to_string()),
                        None => None,
                    });
                    is_match = sub_match(&res, vs) && is_match;
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
                            if !is_match {
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

    pub fn apply<T>(&self, msg: &Message<T>) -> Result<bool>
    where
        T: MessageOwner,
    {
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
                    return Err(UnspecifiedError);
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
                .spawn()?;
        }
        Ok(true)
    }
}

pub fn filter(
    db: &Database,
    query_tag: &str,
    filters: &[Filter],
) -> Result<usize> {
    let q = db.create_query(&format!("tag:{}", query_tag))?;
    let mut msgs = q.search_messages()?;
    let mut matches = 0;
    while let Some(msg) = msgs.next() {
        for filter in filters {
            if filter.apply_if_match(&msg)? {
                matches += 1
            }
        }
        msg.remove_tag(query_tag);
    }
    Ok(matches)
}

pub fn filter_dry(
    db: &Database,
    query_tag: &str,
    filters: &[Filter],
) -> Result<(usize, Vec<String>)> {
    let q = db.create_query(&format!("tag:{}", query_tag))?;
    let mut msgs = q.search_messages()?;
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

pub fn filter_with_path<P>(
    db: P,
    query: &str,
    filters: &[Filter],
) -> Result<usize>
where
    P: AsRef<Path>,
{
    let db = Database::open(&db, DatabaseMode::ReadWrite)?;
    filter(&db, query, filters)
}

pub fn filter_dry_with_path<P>(
    db: P,
    query: &str,
    filters: &[Filter],
) -> Result<(usize, Vec<String>)>
where
    P: AsRef<Path>,
{
    let db = Database::open(&db, DatabaseMode::ReadWrite)?;
    filter_dry(&db, query, filters)
}

pub fn filters_from(buf: &[u8]) -> Result<Vec<Filter>> {
    serde_json::from_slice::<Vec<Filter>>(&buf)?
        .into_iter()
        .map(|f| f.compile())
        .collect()
}

pub fn filters_from_file<P>(filename: &P) -> Result<Vec<Filter>>
where
    P: AsRef<Path>,
{
    let mut buf = Vec::new();
    let mut file = File::open(filename)?;
    file.read_to_end(&mut buf)?;
    filters_from(&buf)
}
