#![deny(future_incompatible)]

trait Tr {
    fn f(u8) {} //~ ERROR anonymous parameters are deprecated
                //~^ WARN this is valid in the current edition
}

fn main() {}
