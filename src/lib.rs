/*!

This crate provides both a library as well as a standalone binary that can be
used as an "[initial tagging]" system for the [notmuch] email system. As a
standalone tool it integrates with the notmuch hooks and configuration files,
while the library may be integrated into a bigger e-mail client that makes use
of [notmuch-rs].

# What?

notcoal reads JSON files with [regex] patterns, checks an incoming message's
respective header for a match. If an appropriate match is found, it is then able
to add or remove tags, run an arbitrary binary for further processing, or delete
the notmuch database entry and the corresponding file.

Rules can be combined with AND as well as OR.

# Example: a filter in a JSON file

```
[{
    "name": "money",
    "desc": "Money stuff",
    "rules": [
        {"from": "@(real\\.bank|gig-economy\\.career)",
         "subject": ["report", "month" ]},
        {"from": "no-reply@trusted\\.bank",
         "subject": "statement"}
    ],
    "op": {
        "add": "€£$",
        "rm": ["inbox", "unread"],
        "run": ["any-binary-in-our-path-or-absolute-path", "--argument"]
    }
}]
```

The rules in this filter definition are equivalent to:

```
( from: ("@real.bank" OR "@gig-economy.career") AND
  subject: ("report" AND "month") )
OR
( from: "no-reply@trusted.bank" AND
  subject: "statement" )
```

If if this filter is applied the operations will

* add the tag `€£$`
* remove the tags `inbox` and `unread`
* run the equivalent of
  `/bin/sh -c 'any-binary-in-our-path-or-absolute-path --argument'`
  with 3 additional environment variables:

```
NOTCOAL_FILTER_NAME=money
NOTCOAL_FILE_NAME=/path/to/maildir/new/filename
NOTCOAL_MSG_ID=e81cadebe7dab1cc6fac7e6a41@some-isp
```

# What notcoal can match

Arbitrary headers! Matching `from` and `subject` are in no way a special case
since all headers are treated equal (and case-insensitive). The mere existence
of a header may be occasionally enough for classification, and while the
[`Value`] enum also has a boolean field, it can not be used in rules.

In addition to arbitrary headers, notcoal also supports "special field checks":

* `@tags`: tags that have already been set by an filter that matched earlier
* `@path`: the file system path of the message being processed
* `@attachment`: any attachment file names
* `@body`: the message body. The first (usually plain text) body part only.
* `@attachment-body`: any attachments contents as long as the MIME type starts
  with `text`
* `@thread-tags`: match on any tag in the thread that we belong to (e.g.
  *mute*).<br>
  **Please note, this applies to the *entire* thread**, not only to the local
  branch.

[regex]: https://docs.rs/regex/
[notmuch]: https://notmuchmail.org/
[initial tagging]: https://notmuchmail.org/initial_tagging/
[notmuch-rs]: https://github.com/vhdirk/notmuch-rs/
[`Value`]: enum.Value.html
*/

extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate mailparse;
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

use mailparse::*;
use notmuch::{
    Database, DatabaseMode, Message, MessageOwner, Query, StreamingIterator,
    Threads,
};
use regex::Regex;

pub mod error;
use error::Error::*;
use error::Result;

/// Possible values for operations and rules
///
/// To make the JSON files more legible in case they are hand-crafted, provide
/// different options for the same fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Value {
    Single(String),
    Multiple(Vec<String>),
    Bool(bool),
}

use Value::*;

/// Operations filters can apply.
///
/// Just a way to store operations, implementation may be found in
/// [`Filter::apply`].
///
/// [`Filter::apply`]: struct.Filter.html#method.apply
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Operations {
    /// Remove tags
    pub rm: Option<Value>,
    /// Add tags
    pub add: Option<Value>,
    /// Run arbitrary commands
    pub run: Option<Vec<String>>,
    /// Delete from disk and notmuch database
    pub del: Option<bool>,
}

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
                    Single(re) => res.push(Regex::new(&re)?),
                    Multiple(mre) => {
                        for re in mre {
                            res.push(Regex::new(&re)?);
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

    /// Combines [`Filter::is_match`] and [`Filter::apply`]
    ///
    /// [`Filter::is_match`]: struct.Filter.html#method.is_match
    /// [`Filter::apply`]: struct.Filter.html#method.apply
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
    /// in [`Filter::rules`]
    ///
    /// [`Filter::rules`]: struct.Filter.html#structfield.rules
    pub fn is_match<T>(&self, msg: &Message<T>, db: &Database) -> Result<bool>
    where
        T: MessageOwner,
    {
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

        /// Check if the supplied part is an attachment and return information
        /// about the content disposition if so
        fn handle_attachment(
            part: &ParsedMail,
        ) -> Result<Option<ParsedContentDisposition>> {
            let cd = part.get_content_disposition()?;
            match cd.disposition {
                DispositionType::Attachment => Ok(Some(cd)),
                _ => Ok(None),
            }
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
                let mut r: Threads<Query>;
                if part == "@path" {
                    // XXX we might want to return an error here if we can't
                    // make the path to a valid utf-8 str? Or maybe go for
                    // to_str_lossy?
                    let vs = msg.filenames().filter_map(|f| match f.to_str() {
                        Some(n) => Some(n.to_string()),
                        None => None,
                    });
                    is_match = sub_match(&res, vs) && is_match;
                } else if part == "@tags" {
                    is_match = sub_match(&res, msg.tags()) && is_match;
                } else if part == "@thread-tags" {
                    // creating a new query as we don't have information about
                    // our own thread yet
                    q = db
                        .create_query(&format!("thread:{}", msg.thread_id()))?;
                    r = q.search_threads()?;
                    if let Some(thread) = r.next() {
                        is_match = sub_match(&res, thread.tags()) && is_match;
                    }
                } else if part == "@attachment"
                    || part == "@attachment-body"
                    || part == "@body"
                {
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
                            .map(|s| match handle_attachment(s)? {
                                Some(cd) => {
                                    Ok(cd.params.get("filename").cloned())
                                }
                                _ => Ok(None),
                            }).collect::<Result<Vec<Option<String>>>>()?;
                        let fns = fns.iter().filter_map(|f| f.clone());
                        is_match = sub_match(&res, fns) && is_match;
                    } else if part == "@body" {
                        is_match = sub_match(&res, [parsed.get_body()?].iter())
                            && is_match;
                    } else if part == "@attachment-body" {
                        let bodys = parsed
                            .subparts
                            .iter()
                            .map(|s| match handle_attachment(s)? {
                                Some(_) => {
                                    // XXX are we sure we only care about text
                                    // mime types? There others?
                                    if s.ctype.mimetype.starts_with("text") {
                                        Ok(Some(s.get_body()?))
                                    } else {
                                        Ok(None)
                                    }
                                }
                                _ => Ok(None),
                            }).collect::<Result<Vec<Option<String>>>>()?;
                        let bodys = bodys.iter().filter_map(|f| f.clone());
                        is_match = sub_match(&res, bodys) && is_match;
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
                    Err(e) => return Err(NotmuchError(e)),
                }
            }
            if is_match {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Apply the operations defined in [`Filter::op`] to the supplied message
    /// regardless if matches this filter or not
    ///
    /// [`Filter::op`]: struct.Filter.html#structfield.op
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
                    return Err(UnsupportedValue(
                        "'add' operation doesn't support bool types"
                            .to_string(),
                    ));
                }
            }
        }
        if let Some(argv) = &self.op.run {
            Command::new(&argv[0])
                .args(&argv[1..])
                .stdout(Stdio::inherit())
                .env("NOTCOAL_FILE_NAME", &msg.filename())
                .env("NOTCOAL_MSG_ID", &msg.id())
                .env("NOTCOAL_FILTER_NAME", &self.name())
                .spawn()?;
        }
        if let Some(del) = &self.op.del {
            if *del {
                // This file was just indexed, so we assume it exists - or do
                // we? See XXX-file
                remove_file(&msg.filename())?;
                db.remove_message(&msg.filename())?;
            }
        }
        Ok(true)
    }
}

/// Very basic sanitisation for our (user supplied) query
fn validate_query_tag(tag: &str) -> Result<String> {
    if tag.is_empty() {
        let e = "Tag to query can't be empty".to_string();
        return Err(UnsupportedQuery(e));
    };
    if tag.contains(' ') || tag.contains('"') || tag.contains('\'') {
        let e = "Query tags can't contain whitespace or quotes".to_string();
        Err(UnsupportedQuery(e))
    } else {
        Ok(format!("tag:{}", tag))
    }
}

/// Apply all supplied filters to the corresponding matching messages
///
/// Either fails or returns how many filters were applied
pub fn filter(
    db: &Database,
    query_tag: &str,
    filters: &[Filter],
) -> Result<usize> {
    let query = validate_query_tag(query_tag)?;
    let q = db.create_query(&query)?;
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
/// matched by which filters, without running any of the operations
pub fn filter_dry(
    db: &Database,
    query_tag: &str,
    filters: &[Filter],
) -> Result<(usize, Vec<String>)> {
    let query = validate_query_tag(query_tag)?;
    let q = db.create_query(&query)?;
    let mut msgs = q.search_messages()?;
    let mut matches = 0;
    let mut mtchinf = Vec::<String>::new();
    while let Some(msg) = msgs.next() {
        let mut msg_matches = 0;
        match filters
            .iter()
            .map(|f| {
                let is_match = f.is_match(&msg, &db)?;
                if is_match {
                    msg_matches += 1;
                    mtchinf.push(format!("{}: {}", msg.id(), f.name()));
                }
                Ok(())
            }).collect::<Result<Vec<()>>>()
        {
            Ok(_) => matches += msg_matches,
            Err(e) => return Err(e),
        };
    }
    Ok((matches, mtchinf))
}

/// Filters messages returned by the query, but takes a database path rather
/// than a `notmuch::Database`
pub fn filter_with_path<P>(
    db: &P,
    query_tag: &str,
    filters: &[Filter],
) -> Result<usize>
where
    P: AsRef<Path>,
{
    let db = Database::open(db, DatabaseMode::ReadWrite)?;
    filter(&db, query_tag, filters)
}

/// Does a dry-run on messages but takes a database path rather than a
/// `notmuch::Database`
pub fn filter_dry_with_path<P>(
    db: &P,
    query_tag: &str,
    filters: &[Filter],
) -> Result<(usize, Vec<String>)>
where
    P: AsRef<Path>,
{
    let db = Database::open(db, DatabaseMode::ReadWrite)?;
    filter_dry(&db, query_tag, filters)
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
