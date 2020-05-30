use std::fs::remove_file;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::Error::*;
use crate::error::*;
use crate::Value;
use crate::Value::*;

use notmuch::{Database, Message, MessageOwner};

/// Operations filters can apply.
///
/// Just a way to store operations, implementation may be found in
/// [`Operations::apply`].
///
/// [`Operations::apply`]: struct.Operations.html#method.apply
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

impl Operations {
    /// Apply the operations defined in [`Filter::op`] to the supplied message
    /// regardless if matches this filter or not
    ///
    /// Operations can fail, but if not they let you know if the message's file
    /// was deleted and dropped from the database.
    ///
    /// If operations have both `run` and `del` defined, the command is run
    /// before the message is deleted.
    ///
    /// [`Filter::op`]: struct.Filter.html#structfield.op
    pub fn apply<T>(
        &self,
        msg: &Message<'_, T>,
        db: &Database,
        name: &str,
    ) -> Result<bool>
    where
        T: MessageOwner,
    {
        if let Some(rm) = &self.rm {
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
        if let Some(add) = &self.add {
            match add {
                Single(tag) => {
                    msg.add_tag(tag)?;
                }
                Multiple(tags) => {
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
        if let Some(argv) = &self.run {
            Command::new(&argv[0])
                .args(&argv[1..])
                .stdout(Stdio::inherit())
                .env("NOTCOAL_FILE_NAME", &msg.filename())
                .env("NOTCOAL_MSG_ID", msg.id().as_ref())
                .env("NOTCOAL_FILTER_NAME", name)
                .spawn()?;
        }
        if let Some(del) = &self.del {
            if *del {
                // This file was just indexed, so we assume it exists - or do
                // we? See XXX-file in filter.rs
                remove_file(&msg.filename())?;
                db.remove_message(&msg.filename())?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}
