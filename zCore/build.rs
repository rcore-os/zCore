extern crate cc;

fn main() {
    println!("cargo:rerun-if-env-changed=LOG");
}
