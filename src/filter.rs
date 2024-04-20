use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::convert::AsRef;
use std::fs::File;
use std::hash::Hasher;
use std::io::Read;
use std::iter::Iterator;

use mailparse::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::Error::*;
use crate::error::*;

use crate::Operations;
use crate::Value;
use crate::Value::*;

use notmuch::{Database, Message, Query, Threads};

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    name: Option<String>,
    /// Description
    ///
    /// Not really used for anything at this point in time, but may be good for
    /// users to remember what this specific filter is doing
    pub desc: Option<String>,
    /// List of rules
    ///
    /// This list is an OR list, meaning the filter will match if any rule
    /// matches. However, AND combinations may happen within a rule
    // at the moment, since we are generating a hash in the name function this
    // field needs to be consistent in the order it prints its key/value pairs
    pub rules: Vec<BTreeMap<String, Value>>,
    /// Operations that will be applied if this any rule matches
    pub op: Operations,
    #[serde(skip)]
    re: Vec<HashMap<String, Vec<Regex>>>,
}

impl Filter {
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns either the set name, or a hash of [`Filter::rules`]. Please
    /// note: hashed names are not used for serialization.
    ///
    /// [`Filter::rules`]: struct.Filter.html#structfield.rules
    pub fn name(&self) -> String {
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
    /// the regular expressions contained in [`Filter::rules`] need to be
    /// compiled before any matches are to be made.
    ///
    /// [`Filter::rules`]: struct.Filter.html#structfield.rules
    pub fn compile(mut self) -> Result<Self> {
        for rule in &self.rules {
            let mut compiled = HashMap::new();
            for (key, value) in rule.iter() {
                let mut res = Vec::new();
                match value {
                    Single(re) => res.push(Regex::new(re)?),
                    Multiple(mre) => {
                        for re in mre {
                            res.push(Regex::new(re)?);
                        }
                    }
                    _ => {
                        let e = "Not a regular expression".to_string();
                        return Err(UnsupportedValue(e));
                    }
                }
                compiled.insert(key.to_string(), res);
            }
            self.re.push(compiled);
        }
        Ok(self)
    }

    /// Combines [`Filter::is_match`] and [`Operations::apply`]
    ///
    /// Returns a tuple of two bools, the first representing if the filter has
    /// been applied, the second if the operation deleted the message that was
    /// supplied
    ///
    /// [`Filter::is_match`]: struct.Filter.html#method.is_match
    /// [`Operations::apply`]: struct.Operations.html#method.apply
    pub fn apply_if_match(&self, msg: &Message, db: &Database) -> Result<(bool, bool)> {
        if self.is_match(msg, db)? {
            Ok((true, self.op.apply(msg, db, &self.name())?))
        } else {
            Ok((false, false))
        }
    }

    /// Checks if the supplied message matches any of the combinations described
    /// in [`Filter::rules`]
    ///
    /// [`Filter::rules`]: struct.Filter.html#structfield.rules
    pub fn is_match(&self, msg: &Message, db: &Database) -> Result<bool> {
        /// Test if any of the supplied values match any of our supplied regular
        /// expressions.
        fn sub_match<I, S>(res: &[Regex], values: I) -> bool
        where
            S: AsRef<str>,
            I: Iterator<Item = S>,
        {
            for value in values {
                for re in res {
                    if re.is_match(value.as_ref()) {
                        return true;
                    }
                }
            }
            false
        }

        // self.re will only be populated after self.compile()
        if self.re.len() != self.rules.len() {
            let e = "Filters need to be compiled before tested".to_string();
            return Err(RegexUncompiled(e));
        }

        for rule in &self.re {
            let mut is_match = true;
            for (part, res) in rule {
                let q: Query;
                let mut r: Threads;
                if part == "@path" {
                    // XXX we might want to return an error here if we can't
                    // make the path to a valid utf-8 str? Or maybe go for
                    // to_str_lossy?
                    let vs = msg
                        .filenames()
                        .filter_map(|f| f.to_str().map(|n| n.to_string()));
                    is_match = sub_match(res, vs) && is_match;
                } else if part == "@tags" {
                    is_match = sub_match(res, msg.tags()) && is_match;
                } else if part == "@thread-tags" {
                    // creating a new query as we don't have information about
                    // our own thread yet
                    q = db.create_query(&format!("thread:{}", msg.thread_id()))?;
                    r = q.search_threads()?;
                    if let Some(thread) = r.next() {
                        is_match = sub_match(res, thread.tags()) && is_match;
                    }
                } else if part == "@attachment" || part == "@attachment-body" || part == "@body" {
                    // since we might combine these we try avoid parsing the
                    // same file over and over again.
                    let mut buf = Vec::new();
                    // XXX-file notmuch says it returns a random filename if
                    // multiple are present. Question is if the new tag is even
                    // applied to messages we've already seen, do we ever run
                    // into that being a problem at all?
                    let mut file = File::open(msg.filename())?;
                    file.read_to_end(&mut buf)?;
                    let parsed = parse_mail(&buf)?;
                    if part == "@attachment" {
                        // XXX Check if this can be refactored with less cloning
                        let fns = parsed
                            .subparts
                            .iter()
                            .map(|s| s.get_content_disposition().params.get("filename").cloned())
                            .collect::<Vec<Option<String>>>();
                        let fns = fns.iter().filter_map(|f| f.clone());
                        is_match = sub_match(res, fns) && is_match;
                    } else if part == "@body" {
                        is_match = sub_match(res, [parsed.get_body()?].iter()) && is_match;
                    } else if part == "@attachment-body" {
                        let bodys = parsed
                            .subparts
                            .iter()
                            .map(|s| {
                                // XXX are we sure we only care about text
                                // mime types? There others?
                                if s.ctype.mimetype.starts_with("text") {
                                    Ok(Some(s.get_body()?))
                                } else {
                                    Ok(None)
                                }
                            })
                            .collect::<Result<Vec<Option<String>>>>()?;
                        let bodys = bodys.iter().filter_map(|f| f.clone());
                        is_match = sub_match(res, bodys) && is_match;
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
                            is_match = re.is_match(&p) && is_match;
                            if !is_match {
                                break;
                            }
                        }
                    }
                    Err(e) => return Err(NotmuchError(e)),
                }
            }
            if is_match {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
