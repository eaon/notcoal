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

```json,ignore
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

```text,ignore
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

```sh,ignore
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

use mailparse;
use notmuch;
use regex;
use serde_derive::{Deserialize, Serialize};
use serde_json;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use notmuch::{Database, DatabaseMode, StreamingIterator};

pub mod error;
use crate::error::Error::*;
use crate::error::Result;
mod filter;
pub use crate::filter::*;
mod operations;
pub use crate::operations::*;

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
        let mut exists = true;
        for filter in filters {
            let (applied, deleted) = filter.apply_if_match(&msg, db)?;
            if applied {
                matches += 1;
            }
            if deleted {
                exists = !deleted;
                break;
            }
        }
        if exists {
            msg.remove_tag(query_tag)?;
        }
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
            })
            .collect::<Result<Vec<()>>>()
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
