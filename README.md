notcoal - notmuch filters, not made from (char)coal
===================================================

[![Latest release](https://img.shields.io/crates/v/notcoal.svg)](https://crates.io/crates/notcoal)
[![Docs](https://docs.rs/notcoal/badge.svg)](https://docs.rs/notcoal/)
[![License](https://img.shields.io/crates/l/notcoal.svg)](https://ghom.niij.org/eaon/notcoal/src/master/LICENSE)

`notcoal` provides both a library as well as a standalone binary. The latter can be used as an
"[initial tagging]" system, the former may be integrated into a bigger e-mail client making use of
[notmuch-rs].

  [initial tagging]: https://notmuchmail.org/initial_tagging/
  [notmuch-rs]: https://github.com/vhdirk/notmuch-rs/

What?
-----

Takes [regex] rules from a JSON file and if any match, either adds new tags, removes tags or runs an
arbitrary binaries for further processing. Rules support AND as well as OR operations.

  [regex]: https://github.com/rust-lang/regex/

Example
-------

```json
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

Here the matching expands to:
```
( from: ("@real.bank" OR "@gig-economy.career") AND subject: ("report" AND "month") )
OR
( from: "no-reply@trusted.bank" AND subject: "statement" )
```

If if this filter is applied the operations will

* add the tag `€£$`
* remove the tags `inbox` and `unread`
* run the equivalent of `/bin/sh -c 'any-binary-in-our-path-or-absolute-path --argument'`
  with 3 additional environment variables:

```sh
NOTCOAL_FILTER_NAME=money
NOTCOAL_FILE_NAME=/path/to/maildir/new/filename
NOTCOAL_MSG_ID=e81cadebe7dab1cc6fac7e6a41@some-isp
```

Filters aren't only restricted to matching `from` and `subject` headers (all of which are treated
case-insensitive) but may try to match arbitrary headers.

Additionally there are the special match fields:

* `@path`: matches on the full path of a message
* `@tags`: matches on tags already set by previous filters
* `@thread-tags`: matches on tags already present in the message's thread
* `@attachment`: matches on an attachment name
* `@attachment-body`: matches on every `text/plain` attachment's body
* `@body`: matches on the messages body

The default `notcoal::filter` function loops through messages and then tests/applies filters in the
order they have been defined in. Hence, any tag one wants to match on has to have been set by a
previous matching rule.

Standalone use for "initial tagging"
------------------------------------

To install the standalone helper binary, the simplest way is:

`cargo install --locked notcoal --features=standalone`

`notcoal` will use the same default database as notmuch itself, and the default location for the
rules file is in `$notmuchdb/.notmuch/hooks/notcoal-rules.json`. It also expects all newly added
messages (that are to be filtered) to have the `new` tag. To make sure that's being set, edit your
`.notmuch-config` to include:

```ini
[new]
tags=unread;inbox;new;
```

Additionally, `notcoal` will respect the config file's maildir synchronize setting.

See `notcoal --help` for supplying alternative values.

If you're fine with the defaults, you can symlink `$notmuchdb/.notmuch/hooks/post-new` to the
`notcoal` binary.

Thanks
------

[vhdirk] for `notmuch-rs`, which made this crate possible in the first place, [korrat] for a patch
for more sensible database discovery, [antifuchs' gmail-britta][britta] for inspiring the name, and
[Recurse Center], for creating a supportive environment 💟

  [vhdirk]: https://github.com/vhdirk/
  [korrat]: https://korr.at/
  [britta]: https://github.com/antifuchs/gmail-britta/
  [Recurse Center]: https://www.recurse.com/
