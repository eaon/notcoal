extern crate notcoal;

use notcoal::Value::*;
use notcoal::Operation;

fn main() {
    notcoal::load_config(None);
    let filters = match notcoal::filters_from_file("example-rules.json") {
        Ok(f) => f,
        Err(e) => {
            println!("{:?}", e);
            Vec::new()
        }
    };

    for filter in filters {
        match filter.op {
            Operation::Rm(Single(ref tag)) => println!("{}", tag),
            Operation::Add(Single(ref tag)) => println!("{}", tag),
            Operation::Rm(Multiple(ref tags)) => {
                for tag in tags {
                    println!("{}", tag);
                }
            },
            Operation::Rm(Bool(ref all)) => println!("remove all tags: {:?}", all),
            _ => {}
        }
    }
}
