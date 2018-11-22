//! # notcoal - filters for a notmuch mail environment
//!
//! This crate provides both a library as well as a standalone binary to
//!
//! Example rule file:
//! ```json
//! [{
//!     "name": "money",
//!     "desc": "Money stuff",
//!     "rules": [
//!         {"from": "@(real\\.bank|gig-economy\\.career)",
//!          "subject": ["report", "month" ]},
//!         {"from": "no-reply@trusted\\.bank",
//!          "subject": "statement"}
//!     ],
//!     "op": {
//!         "add": "€£$",
//!         "rm": ["inbox", "unread"],
//!         "run": ["any-binary-in-our-path-or-absolute-path", "--argument"]
//!     }
//! }]
//! ```

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
use std::fs::remove_file;
use std::fs::File;
use std::hash::Hasher;
use std::io::Read;
use std::iter::Iterator;
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;

use notmuch::{
    Database, DatabaseMode, Message, MessageOwner, Query, StreamingIterator,
    Threads,
};

pub mod error;
use error::Error::*;
use error::Result;

/// To make the `.json` files more legible in case they are hand-crafted,
/// provide different options for the same fields when it makes sense for them
/// to be flexible.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Value {
    Single(String),
    Multiple(Vec<String>),
    Bool(bool),
}

use Value::*;

/// Operations filters can
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub rm: Option<Value>,
    pub add: Option<Value>,
    pub run: Option<Vec<String>>,
    pub del: Option<bool>,
}

/// Everything this crate is built around
#[derive(Debug, Serialize, Deserialize, Default)]
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

impl Filter {
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns either the set name, or a hash of `Filter::rules`
    ///
    /// Please note: hashed names are not used for serialization.
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

    /// When filters are deserialized from json or have been assembled via code,
    /// the regular expressions contained in `Filter::rules` need to be compiled
    /// before any matches are to be made.
    pub fn compile(mut self) -> Result<Self> {
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

    /// Combines `Filter::is_match` and `Filter::apply`
    pub fn apply_if_match<T>(
        &self,
        msg: &Message<T>,
        db: &Database,
    ) -> Result<bool>
    where
        T: MessageOwner,
    {
        if self.is_match(msg, db)? {
            Ok(self.apply(msg, db)?)
        } else {
            Ok(false)
        }
    }

    /// Checks if the supplied message matches any of the combinations described
    /// in `Filter::rules`
    pub fn is_match<T>(&self, msg: &Message<T>, db: &Database) -> Result<bool>
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

        // XXX Maybe return a Result here? If we haven't compiled rules, return
        // Err instead of false - would change the type signature for the return
        if &self.re.len() != &self.rules.len() {
            return Ok(false);
        }

        for rule in &self.re {
            let mut is_match = true;
            for (part, res) in rule {
                let q: Query;
                let mut r: Threads<Query>;
                if part == "@path" {
                    let vs = msg.filenames().filter_map(|f| match f.to_str() {
                        Some(n) => Some(n.to_string()),
                        None => None,
                    });
                    is_match = sub_match(&res, vs) && is_match;
                } else if part == "@tags" {
                    is_match = sub_match(&res, msg.tags()) && is_match;
                } else if part == "@thread-tags" {
                    q = db
                        .create_query(&format!("thread:{}", msg.thread_id()))?;
                    r = q.search_threads()?;
                    if let Some(thread) = r.next() {
                        is_match = sub_match(&res, thread.tags()) && is_match;
                    }
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
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Apply the operations defined in `Filter::op` to the supplied message
    /// regardless if matches this filter or not
    pub fn apply<T>(&self, msg: &Message<T>, db: &Database) -> Result<bool>
    where
        T: MessageOwner,
    {
        if let Some(rm) = &self.op.rm {
            match rm {
                Single(tag) => {
                    msg.remove_tag(tag)?;
                }
                Multiple(tags) => {
                    for tag in tags {
                        msg.remove_tag(tag)?;
                    }
                }
                Bool(all) => {
                    if *all {
                        msg.remove_all_tags()?;
                    }
                }
            }
        }
        if let Some(add) = &self.op.add {
            match add {
                Single(ref tag) => {
                    msg.add_tag(tag)?;
                }
                Multiple(ref tags) => {
                    for tag in tags {
                        msg.add_tag(tag)?;
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
        if let Some(del) = &self.op.del {
            if *del {
                // This file was just indexed, so we assume it exists
                remove_file(&msg.filename())?;
                db.remove_message(&msg.filename())?;
            }
        }
        Ok(true)
    }
}

/// Helper function that "does it all"
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
            if filter.apply_if_match(&msg, db)? {
                matches += 1
            }
        }
        msg.remove_tag(query_tag)?;
    }
    Ok(matches)
}

/// Returns how many matches there are as well as what Message-IDs have been
/// matched by which filters
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
            if f.is_match(&msg, &db).unwrap() {
                mtchinf.push(format!("{}: {}", msg.id(), f.get_name()));
                a += 1
            }
            a
        });
    }
    Ok((matches, mtchinf))
}

/// Filters messages returned by the query, but takes a database path rather
/// than a `notmuch::Database`
pub fn filter_with_path<P>(
    db: &P,
    query: &str,
    filters: &[Filter],
) -> Result<usize>
where
    P: AsRef<Path>,
{
    let db = Database::open(db, DatabaseMode::ReadWrite)?;
    filter(&db, query, filters)
}

/// Does a dry-run on messages but takes a database path rather than a
/// `notmuch::Database`
pub fn filter_dry_with_path<P>(
    db: &P,
    query: &str,
    filters: &[Filter],
) -> Result<(usize, Vec<String>)>
where
    P: AsRef<Path>,
{
    let db = Database::open(db, DatabaseMode::ReadWrite)?;
    filter_dry(&db, query, filters)
}

/// Deserialize filters from bytes
pub fn filters_from(buf: &[u8]) -> Result<Vec<Filter>> {
    serde_json::from_slice::<Vec<Filter>>(&buf)?
        .into_iter()
        .map(|f| f.compile())
        .collect()
}

/// Deserialize a filters from file
pub fn filters_from_file<P>(filename: &P) -> Result<Vec<Filter>>
where
    P: AsRef<Path>,
{
    let mut buf = Vec::new();
    let mut file = File::open(filename)?;
    file.read_to_end(&mut buf)?;
    filters_from(&buf)
}
